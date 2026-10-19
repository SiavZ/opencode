//! Purpose: OpenAI provider facade and OpenAI option defaults (packages/llm/src/providers/openai.ts,
//! openai-options.ts): Responses (HTTP + WebSocket) and Chat routes, bearer auth from
//! `apiKey`/`OPENAI_API_KEY`, `store:false` and GPT-5 reasoning defaults.
//! Scope: route configuration and model construction. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/options.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/config.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! |-- openai::chat_route
//! |-- openai::responses_route
//! |-- openai::responses_websocket_route
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/tests/providers_facades.rs
//! Invariants: user provider options merge over the computed defaults (`mergeProviderOptions`).
//! Contract tests: provider-adapters/tests/providers_facades.rs
//!
//! Example: `openai::configure(cfg, transports).responses("gpt-5")` sends
//! `providerOptions.openai = { store:false, reasoningEffort:"medium", reasoningSummary:"auto",
//! include:["reasoning.encrypted_content"], textVerbosity:"low" (only for gpt-5.x) }`.

use std::sync::Arc;

use opencode_llm_api::options::merge_provider_options;
use opencode_llm_api::{JsonObject, Model, ProviderOptions, RouteDefaults};
use serde_json::{Value, json};

use crate::protocols::openai;
use crate::providers::config::{ProviderConfig, Transports, apply_defaults, patch_endpoint};
use crate::route::ProtocolRoute;

pub const ID: &str = "openai";
pub const ENV: &[&str] = &["OPENAI_API_KEY"];

/// `gpt5DefaultOptions(modelID, { textVerbosity })`.
pub fn gpt5_default_options(model_id: &str, text_verbosity: bool) -> Option<ProviderOptions> {
    let id = model_id.to_lowercase();
    if !id.contains("gpt-5") || id.contains("gpt-5-chat") || id.contains("gpt-5-pro") {
        return None;
    }
    let mut openai = JsonObject::new();
    openai.insert("reasoningEffort".into(), json!("medium"));
    openai.insert("reasoningSummary".into(), json!("auto"));
    openai.insert("include".into(), json!(["reasoning.encrypted_content"]));
    if text_verbosity && id.contains("gpt-5.") && !id.contains("codex") && !id.contains("-chat") {
        openai.insert("textVerbosity".into(), json!("low"));
    }
    Some(ProviderOptions::from([("openai".to_owned(), openai)]))
}

/// `openAIDefaultOptions`: `{ openai: { store: false } }` merged with the GPT-5 defaults.
pub fn openai_default_options(model_id: &str, text_verbosity: bool) -> Option<ProviderOptions> {
    let store = ProviderOptions::from([(
        "openai".to_owned(),
        JsonObject::from_iter([("store".to_owned(), Value::Bool(false))]),
    )]);
    let gpt5 = gpt5_default_options(model_id, text_verbosity);
    merge_provider_options([Some(&store), gpt5.as_ref()])
}

/// `withOpenAIOptions(modelID, options, defaults)`: defaults < user provider options.
pub fn with_openai_options(
    model_id: &str,
    defaults: &RouteDefaults,
    text_verbosity: bool,
) -> RouteDefaults {
    let computed = openai_default_options(model_id, text_verbosity);
    RouteDefaults {
        provider_options: merge_provider_options([
            computed.as_ref(),
            defaults.provider_options.as_ref(),
        ]),
        ..defaults.clone()
    }
}

/// Bind a configured route to a model id with OpenAI option defaults.
pub fn model_with_options(
    route: &ProtocolRoute,
    defaults: &RouteDefaults,
    provider: &str,
    model_id: &str,
    text_verbosity: bool,
) -> Model {
    let mut route = route.clone();
    apply_defaults(
        &mut route,
        &with_openai_options(model_id, defaults, text_verbosity),
    );
    Model::new(model_id, provider, Arc::new(route))
}

/// The configured OpenAI facade (`OpenAI.configure(input)`).
#[derive(Clone, Debug)]
pub struct OpenAIProvider {
    pub responses_route: ProtocolRoute,
    pub websocket_route: ProtocolRoute,
    pub chat_route: ProtocolRoute,
    pub defaults: RouteDefaults,
}

pub fn configure(config: &ProviderConfig, transports: &Transports) -> OpenAIProvider {
    let auth = config.bearer(ENV);
    let configured = |mut route: ProtocolRoute| {
        route.auth = auth.clone();
        patch_endpoint(&mut route, config.base_url.as_deref(), &config.query);
        route
    };
    OpenAIProvider {
        responses_route: configured(openai::responses_route(transports.sse.clone())),
        websocket_route: configured(openai::responses_websocket_route()),
        chat_route: configured(openai::chat_route(transports.sse.clone())),
        defaults: config.defaults.clone(),
    }
}

impl OpenAIProvider {
    /// `model` is `responses`.
    pub fn model(&self, id: &str) -> Model {
        self.responses(id)
    }

    pub fn responses(&self, id: &str) -> Model {
        model_with_options(&self.responses_route, &self.defaults, ID, id, true)
    }

    pub fn responses_websocket(&self, id: &str) -> Model {
        model_with_options(&self.websocket_route, &self.defaults, ID, id, true)
    }

    pub fn chat(&self, id: &str) -> Model {
        model_with_options(&self.chat_route, &self.defaults, ID, id, false)
    }
}
