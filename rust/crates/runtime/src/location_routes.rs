//! Purpose: bind resolved Location model specs to native provider protocol transports.
//! Scope: Location-scoped route factory, with no provider I/O until a runner turn.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- session-runtime/src/runner/model.rs
//! External:
//! |-- anthropic::route
//! |-- openai::compatible_chat_route
//! `-- openai::responses_route
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: credentials remain auth headers, model body overlays never contain apiKey.
//! Contract tests: runtime/tests/production.rs

use std::sync::Arc;

use opencode_llm_api::{HttpOptions, Route};
use opencode_provider_adapters::auth::{CredentialHeader, NoAuth, Secret, StaticCredential};
use opencode_provider_adapters::protocols::{anthropic, openai};
use opencode_provider_adapters::transport::{HttpExecutor, HttpJsonTransport};
use opencode_session_runtime::runner::model::{NativeRoute, RouteAuth, RouteFactory, RouteSpec};

pub struct NativeRoutes(pub Arc<HttpJsonTransport>);

impl NativeRoutes {
    pub fn new() -> Result<Self, opencode_llm_api::LlmError> {
        Ok(Self(Arc::new(HttpJsonTransport::sse(HttpExecutor::new()?))))
    }
}

impl RouteFactory for NativeRoutes {
    fn route(&self, spec: RouteSpec) -> Arc<dyn Route> {
        let mut route = match spec.route {
            NativeRoute::OpenAIResponses => openai::responses_route(self.0.clone()),
            NativeRoute::OpenAICompatibleChat => openai::compatible_chat_route(self.0.clone()),
            NativeRoute::AnthropicMessages => anthropic::route(self.0.clone()),
        };
        if let Some(url) = spec.base_url {
            route.endpoint.base_url = Some(url);
        }
        route.provider = Some(spec.provider);
        route.defaults.headers = Some(spec.headers);
        route.defaults.limits = Some(spec.limits);
        route.defaults.http = Some(HttpOptions {
            body: Some(spec.body),
            ..HttpOptions::default()
        });
        route.auth = match spec.auth {
            RouteAuth::None => Arc::new(NoAuth),
            RouteAuth::Bearer(value) => opencode_provider_adapters::auth::bearer_value(value),
            RouteAuth::Header { name, value } => Arc::new(CredentialHeader::header(
                &name,
                Arc::new(StaticCredential {
                    source: "configured model credential".into(),
                    value: Some(Secret::new(value)),
                }),
            )),
        };
        Arc::new(route)
    }
}
