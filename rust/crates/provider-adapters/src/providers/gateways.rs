//! Purpose: OpenRouter and Cloudflare facades (packages/llm/src/providers/openrouter.ts,
//! cloudflare.ts): the `openrouter-chat` protocol (Chat body plus `usage`/`reasoning`/
//! `prompt_cache_key` options), AI Gateway compat URL and `cf-aig-authorization` auth, Workers AI
//! account URL with bearer auth.
//! Scope: route configuration, protocol decoration and model construction. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/wrap.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/redact.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/src/providers/registry.rs
//! Invariants: OpenRouter options are spread after the Chat body (they may overwrite Chat
//! keys). Gateway id defaults to `default` when blank. An explicit `apiKey` on the gateway
//! sends upstream bearer auth, plus `cf-aig-authorization` only when `gatewayApiKey` is set.
//! Contract tests: provider-adapters/tests/providers_gateways.rs

use std::sync::Arc;

use opencode_llm_api::{LlmError, LlmRequest, Model, ProviderId};
use serde_json::{Value, json};

use crate::auth::{AndThen, CredentialHeader, SharedAuth, bearer_value};
use crate::protocols::openai::{self, OpenAIChatProtocol};
use crate::providers::compatible;
use crate::providers::config::{
    ProviderConfig, Transports, apply_defaults, credential, literal, patch_endpoint,
};
use crate::providers::native::SingleRoute;
use crate::providers::wrap::Decorated;
use crate::route::{Endpoint, ProtocolRoute};
use crate::transport::redact::encode_uri_component;

pub const OPENROUTER_ID: &str = "openrouter";
pub const OPENROUTER_ENV: &[&str] = &["OPENROUTER_API_KEY"];
pub const AI_GATEWAY_ID: &str = "cloudflare-ai-gateway";
pub const WORKERS_AI_ID: &str = "cloudflare-workers-ai";
pub const AI_GATEWAY_ENV: &[&str] = &["CLOUDFLARE_API_TOKEN", "CF_AIG_TOKEN"];
pub const WORKERS_AI_ENV: &[&str] = &["CLOUDFLARE_API_KEY", "CLOUDFLARE_WORKERS_AI_TOKEN"];

/// `bodyOptions(request.providerOptions.openrouter)`.
pub fn openrouter_body_options(request: &LlmRequest) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let Some(options) = request
        .provider_options
        .as_ref()
        .and_then(|p| p.get(OPENROUTER_ID))
    else {
        return out;
    };
    match options.get("usage") {
        Some(Value::Bool(true)) => {
            out.insert("usage".into(), json!({ "include": true }));
        }
        Some(Value::Object(usage)) => {
            out.insert("usage".into(), Value::Object(usage.clone()));
        }
        _ => {}
    }
    if let Some(Value::Object(reasoning)) = options.get("reasoning") {
        out.insert("reasoning".into(), Value::Object(reasoning.clone()));
    }
    if let Some(Value::String(key)) = options.get("promptCacheKey") {
        out.insert("prompt_cache_key".into(), Value::String(key.clone()));
    }
    out
}

/// `OpenRouter.protocol` (`openrouter-chat`).
pub fn openrouter_protocol() -> Decorated {
    Decorated {
        id: "openrouter-chat",
        inner: Arc::new(OpenAIChatProtocol),
        body: Some(Arc::new(|request, body| {
            let Value::Object(mut body) = body else {
                return Ok(body);
            };
            body.extend(openrouter_body_options(request));
            Ok(Value::Object(body))
        })),
        frame: None,
    }
}

/// `OpenRouter.configure(input)`: route id `openrouter`, profile base URL, bearer auth.
pub fn openrouter(config: &ProviderConfig, transports: &Transports) -> SingleRoute {
    let base = compatible::profile(OPENROUTER_ID).map(|p| p.base_url);
    let mut route = ProtocolRoute::new(
        "openrouter",
        Arc::new(openrouter_protocol()),
        Endpoint::new(base, openai::CHAT_PATH),
        transports.sse.clone(),
    );
    route.provider = Some(ProviderId::new(OPENROUTER_ID));
    route.auth = config.bearer(OPENROUTER_ENV);
    patch_endpoint(&mut route, config.base_url.as_deref(), &config.query);
    apply_defaults(&mut route, &config.defaults);
    SingleRoute {
        id: ProviderId::new(OPENROUTER_ID),
        route,
    }
}

/// `GatewayURL`: `baseURL`, or `accountId` plus optional `gatewayId`.
#[derive(Clone, Debug, Default)]
pub struct CloudflareConfig {
    pub account_id: Option<String>,
    pub gateway_id: Option<String>,
    /// `gatewayApiKey`, sent as `cf-aig-authorization`.
    pub gateway_api_key: Option<String>,
}

/// `aiGatewayBaseURL(input)`.
pub fn ai_gateway_base_url(
    base_url: Option<&str>,
    cf: &CloudflareConfig,
) -> Result<String, LlmError> {
    if let Some(url) = base_url.filter(|u| !u.is_empty()) {
        return Ok(url.to_owned());
    }
    let Some(account) = cf.account_id.as_deref().filter(|a| !a.is_empty()) else {
        return Err(LlmError::invalid_request(
            "CloudflareAIGateway.configure requires accountId unless baseURL is supplied",
        ));
    };
    let gateway = cf
        .gateway_id
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .unwrap_or("default");
    Ok(format!(
        "https://gateway.ai.cloudflare.com/v1/{}/{}/compat",
        encode_uri_component(account),
        encode_uri_component(gateway)
    ))
}

/// `workersAIBaseURL(input)`.
pub fn workers_ai_base_url(
    base_url: Option<&str>,
    account_id: Option<&str>,
) -> Result<String, LlmError> {
    if let Some(url) = base_url.filter(|u| !u.is_empty()) {
        return Ok(url.to_owned());
    }
    let Some(account) = account_id.filter(|a| !a.is_empty()) else {
        return Err(LlmError::invalid_request(
            "CloudflareWorkersAI.configure requires accountId unless baseURL is supplied",
        ));
    };
    Ok(format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/ai/v1",
        encode_uri_component(account)
    ))
}

fn ai_gateway_auth(config: &ProviderConfig, cf: &CloudflareConfig) -> SharedAuth {
    if let Some(auth) = &config.auth {
        return auth.clone();
    }
    let gateway_header = |source| -> SharedAuth {
        Arc::new(CredentialHeader::bearer_header(
            "cf-aig-authorization",
            source,
        ))
    };
    match (&config.api_key, &cf.gateway_api_key) {
        (None, gateway_key) => gateway_header(credential(
            gateway_key.as_deref(),
            AI_GATEWAY_ENV,
            &config.env,
        )),
        (Some(key), None) => bearer_value(key.clone()),
        (Some(key), Some(gateway_key)) => Arc::new(AndThen(
            gateway_header(literal("gatewayApiKey", gateway_key)),
            bearer_value(key.clone()),
        )),
    }
}

fn compat_route(id: &str, config: &ProviderConfig, transports: &Transports) -> ProtocolRoute {
    let mut route = openai::compatible_chat_route(transports.sse.clone());
    route.id = id.to_owned();
    route.provider = Some(ProviderId::new(id));
    apply_defaults(&mut route, &config.defaults);
    route
}

/// `CloudflareAIGateway.configure(options)`.
pub fn ai_gateway(
    config: &ProviderConfig,
    cf: &CloudflareConfig,
    transports: &Transports,
) -> Result<SingleRoute, LlmError> {
    let mut route = compat_route(AI_GATEWAY_ID, config, transports);
    route.endpoint.base_url = Some(ai_gateway_base_url(config.base_url.as_deref(), cf)?);
    route.auth = ai_gateway_auth(config, cf);
    Ok(SingleRoute {
        id: ProviderId::new(AI_GATEWAY_ID),
        route,
    })
}

/// `CloudflareWorkersAI.configure(options)`: `AuthOptions.bearer(input, workersAIAuthEnvVars)`.
pub fn workers_ai(
    config: &ProviderConfig,
    account_id: Option<&str>,
    transports: &Transports,
) -> Result<SingleRoute, LlmError> {
    let mut route = compat_route(WORKERS_AI_ID, config, transports);
    route.endpoint.base_url = Some(workers_ai_base_url(config.base_url.as_deref(), account_id)?);
    route.auth = config.bearer(WORKERS_AI_ENV);
    Ok(SingleRoute {
        id: ProviderId::new(WORKERS_AI_ID),
        route,
    })
}

/// `isWorkersAi` model ids on the Unified API (`workers-ai/...` or bare `@cf/...`).
pub fn is_workers_ai_model(model_id: &str) -> bool {
    model_id.starts_with("workers-ai/") || model_id.starts_with("@cf/")
}

/// Model with the Cloudflare route.
pub fn model(route: &SingleRoute, id: &str) -> Model {
    route.model(id)
}
