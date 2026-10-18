//! Purpose: runnable route contract (`Route`), resolved `Model`, prepared request, route
//! defaults, model resolution port and the request-option precedence applied before compile.
//! Scope: dyn-compatible traits plus pure precedence logic. No transport here.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/ids.rs
//! |-- llm-api/src/message.rs
//! `-- llm-api/src/options.rs
//! External:
//! |-- futures::{future::BoxFuture, stream::BoxStream}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- llm-api/src/client.rs
//! |-- llm-api/src/lib.rs
//! `-- llm-api/src/message.rs
//! Invariants: one `Route::stream` call is exactly one provider turn. Routes never run tool
//! loops or replay a partially consumed stream, status-level retries happen before any event
//! is yielded (packages/llm/src/route/executor.ts). Cancellation is owned by the consumer:
//! dropping the returned stream must abort the transport and leave no detached task.
//! Secrets live only inside the route's auth, `PreparedRequest` and errors never carry
//! credential values. Option precedence is route defaults < model defaults < request
//! (packages/llm/src/route/client.ts `resolveRequestOptions`).
//! Contract tests: llm-api/tests/contract.rs

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;

use crate::error::LlmError;
use crate::event::LlmEvent;
use crate::ids::{JsonObject, ModelId, ProviderId};
use crate::message::LlmRequest;
use crate::options::{
    GenerationOptions, HttpOptions, ModelCompatibility, ModelDefaults, ModelLimits,
    ProviderOptions, merge_generation_options, merge_http_options, merge_provider_options,
};

/// Ordered, fallible provider events for one turn. `Send` so consumers can drive it from
/// any tokio worker.
pub type LlmEventStream = BoxStream<'static, Result<LlmEvent, LlmError>>;

/// Route/request defaults carried by a configured route (`RouteDefaults`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RouteDefaults {
    pub headers: Option<indexmap::IndexMap<String, String>>,
    pub limits: Option<ModelLimits>,
    pub generation: Option<GenerationOptions>,
    pub provider_options: Option<ProviderOptions>,
    pub http: Option<HttpOptions>,
}

/// Compiled, validated provider-native body without sending it (`LLM.PreparedRequest`).
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedRequest {
    pub id: String,
    pub route: String,
    pub protocol: String,
    pub model: ModelId,
    pub provider: ProviderId,
    pub body: Value,
    pub metadata: Option<JsonObject>,
}

/// A configured deployment: protocol + endpoint + auth + framing/transport.
pub trait Route: Send + Sync + fmt::Debug {
    /// Route id used in diagnostics and prepared metadata (e.g. `openai-responses`).
    fn id(&self) -> &str;
    /// Wire protocol id (e.g. `openai-chat`).
    fn protocol(&self) -> &str;
    /// Transport id (e.g. `http-json`, `websocket-json`).
    fn transport(&self) -> &str;
    fn provider(&self) -> Option<&ProviderId>;
    fn defaults(&self) -> &RouteDefaults;
    /// Compile `request` (already option-resolved) into the provider body without I/O.
    fn prepare(&self, request: &LlmRequest) -> Result<PreparedRequest, LlmError>;
    /// Execute one provider turn. The request is already option-resolved by the client.
    fn stream(&self, request: LlmRequest) -> LlmEventStream;
}

/// Selected model bound to a runnable route (`LLM.Model`).
#[derive(Clone)]
pub struct Model {
    pub id: ModelId,
    pub provider: ProviderId,
    pub route: Arc<dyn Route>,
    pub defaults: Option<ModelDefaults>,
    pub compatibility: Option<ModelCompatibility>,
}

impl fmt::Debug for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Model")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("route", &self.route.id())
            .field("defaults", &self.defaults)
            .field("compatibility", &self.compatibility)
            .finish()
    }
}

impl Model {
    pub fn new(
        id: impl Into<ModelId>,
        provider: impl Into<ProviderId>,
        route: Arc<dyn Route>,
    ) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            route,
            defaults: None,
            compatibility: None,
        }
    }

    /// `"{provider}/{route}"`, the label used in stream diagnostics.
    pub fn route_label(&self) -> String {
        format!("{}/{}", self.provider, self.route.id())
    }
}

/// `resolveRequestOptions`: apply route < model < request precedence for generation,
/// provider options and HTTP overlay. Generation is always present afterwards.
pub fn resolve_request_options(mut request: LlmRequest) -> LlmRequest {
    let route = request.model.route.defaults().clone();
    let model = request.model.defaults.clone().unwrap_or_default();
    request.generation = Some(
        merge_generation_options([
            route.generation.as_ref(),
            model.generation.as_ref(),
            request.generation.as_ref(),
        ])
        .unwrap_or_default(),
    );
    request.provider_options = merge_provider_options([
        route.provider_options.as_ref(),
        model.provider_options.as_ref(),
        request.provider_options.as_ref(),
    ]);
    request.http = merge_http_options([
        route.http.as_ref(),
        model.http.as_ref(),
        request.http.as_ref(),
    ]);
    request
}

/// What a consumer asks to run: catalog provider/model plus optional variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: ProviderId,
    pub model: ModelId,
    pub variant: Option<String>,
}

/// Typed failures of model resolution (`SessionRunnerModel` errors in core).
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("Model unavailable: {provider}/{model}")]
    ModelUnavailable {
        provider: ProviderId,
        model: ModelId,
    },
    #[error("Variant unavailable: {provider}/{model} variant {variant}")]
    VariantUnavailable {
        provider: ProviderId,
        model: ModelId,
        variant: String,
    },
    #[error("Unsupported API for {provider}/{model}: {api}")]
    UnsupportedApi {
        provider: ProviderId,
        model: ModelId,
        api: String,
    },
    #[error("No default model available")]
    NoDefaultModel,
    #[error(transparent)]
    Llm(#[from] LlmError),
}

/// Catalog + credential backed model resolution, implemented by provider-adapters.
/// `None` selection resolves the default model.
pub trait ModelResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        selection: Option<&'a ModelSelection>,
    ) -> BoxFuture<'a, Result<Model, ResolveError>>;
}
