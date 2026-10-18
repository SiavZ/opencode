//! Purpose: cloud-account providers resolved from the catalog: Google Vertex (Gemini and
//! Anthropic publishers), Bedrock Converse and Mantle, and Snowflake Cortex
//! (packages/core/src/plugin/provider/{google-vertex,amazon-bedrock,snowflake-cortex}.ts and the
//! matching legacy loaders in packages/opencode/src/provider/provider.ts).
//! Scope: route construction from a `CatalogRoute`. Credential acquisition (ADC, AWS chain) is
//! the credentials module's job; without a supplied `auth` those modes fail typed.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/wrap.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/unsupported.rs
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/registry.rs
//! Invariants: Vertex Anthropic bodies drop `model` and carry `anthropic_version:
//! vertex-2023-10-16`; Snowflake sends `max_completion_tokens` and repairs `"role":""`.
//! Contract tests: provider-adapters/tests/providers_cloud.rs

use std::sync::{Arc, LazyLock};

use opencode_llm_api::{LlmError, ModelId, ProviderId};
use regex::Regex;
use serde_json::Value;

use crate::auth::{Headers, Secret, SharedAuth, bearer_value};
use crate::protocols::bedrock::BedrockCredentials;
use crate::protocols::{anthropic as messages, gemini, openai};
use crate::providers::bedrock::{
    self as bedrock_facade, BedrockConfig, mantle_base_url, mantle_uses_chat, resolve_model_id,
};
use crate::providers::config::{ProviderConfig, Transports};
use crate::providers::inventory::CatalogRoute;
use crate::providers::loaders;
use crate::providers::wrap::Decorated;
use crate::route::{Endpoint, EndpointPath, ProtocolRoute};
use crate::unsupported::not_yet_ported;

pub const VERTEX_EXPRESS_BASE_URL: &str = "https://aiplatform.googleapis.com/v1/publishers/google";
pub const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

fn unported(route: &CatalogRoute, what: &str) -> LlmError {
    not_yet_ported(
        &ProviderId::new(route.provider_id.as_str()),
        &ModelId::new(route.model_id.as_str()),
        what,
    )
}

fn require_auth(route: &CatalogRoute, what: &str) -> Result<SharedAuth, LlmError> {
    route.auth.clone().ok_or_else(|| unported(route, what))
}

/// `@ai-sdk/google-vertex`: Gemini over Vertex. Express mode (`GOOGLE_VERTEX_API_KEY`) uses
/// `x-goog-api-key`; otherwise an ADC bearer `auth` must be supplied.
pub fn vertex_gemini(
    route: &CatalogRoute,
    transports: &Transports,
) -> Result<ProtocolRoute, LlmError> {
    let mut out = gemini::route(transports.sse.clone());
    let express = route
        .option("apiKey")
        .map(str::to_owned)
        .or_else(|| route.env_value("GOOGLE_VERTEX_API_KEY"));
    if let Some(key) = express {
        out.endpoint.base_url = Some(
            route
                .url()
                .map(str::to_owned)
                .unwrap_or_else(|| VERTEX_EXPRESS_BASE_URL.into()),
        );
        out.auth = Arc::new(crate::auth::CredentialHeader::header(
            "x-goog-api-key",
            Arc::new(crate::auth::StaticCredential {
                source: "GOOGLE_VERTEX_API_KEY".into(),
                value: Some(Secret::new(key)),
            }),
        ));
        return Ok(out);
    }
    let location = loaders::vertex_location(route);
    let base = match route.url() {
        Some(url) => url.to_owned(),
        None => {
            let Some(project) = loaders::vertex_project(route) else {
                return Err(LlmError::invalid_request(
                    "Google Vertex project setting is missing. Pass it using the 'project' parameter or the GOOGLE_VERTEX_PROJECT environment variable.",
                ));
            };
            format!(
                "https://{}/v1beta1/projects/{project}/locations/{location}/publishers/google",
                loaders::vertex_host(&location)
            )
        }
    };
    out.endpoint.base_url = Some(base);
    out.auth = require_auth(route, "google-vertex ADC access token")?;
    Ok(out)
}

/// Vertex Anthropic protocol: `transformRequestBody` drops `model`, adds `anthropic_version`.
pub fn vertex_anthropic_protocol() -> Decorated {
    Decorated {
        id: "vertex-anthropic-messages",
        inner: Arc::new(messages::AnthropicMessages),
        body: Some(Arc::new(|_, body| {
            let Value::Object(mut body) = body else {
                return Ok(body);
            };
            body.shift_remove("model");
            body.insert("anthropic_version".into(), VERTEX_ANTHROPIC_VERSION.into());
            Ok(Value::Object(body))
        })),
        frame: None,
    }
}

/// `@ai-sdk/google-vertex/anthropic`: `{base}/{model}:streamRawPredict`.
pub fn vertex_anthropic(
    route: &CatalogRoute,
    transports: &Transports,
) -> Result<ProtocolRoute, LlmError> {
    let location = loaders::vertex_anthropic_location(route);
    let project = loaders::vertex_anthropic_project(route);
    let base = match (route.url(), project) {
        (Some(url), _) => url.to_owned(),
        (None, Some(project)) => format!(
            "https://{}/v1/projects/{project}/locations/{location}/publishers/anthropic/models",
            loaders::vertex_host(&location)
        ),
        (None, None) => {
            return Err(LlmError::invalid_request(
                "Google Vertex project setting is missing. Set GOOGLE_CLOUD_PROJECT or the provider 'project' option.",
            ));
        }
    };
    let mut out = ProtocolRoute::new(
        "vertex-anthropic-messages",
        Arc::new(vertex_anthropic_protocol()),
        Endpoint {
            base_url: Some(base),
            path: EndpointPath::Dynamic(Arc::new(|request, _| {
                format!("/{}:streamRawPredict", request.model.id.as_str().trim())
            })),
            query: Headers::new(),
        },
        transports.sse.clone(),
    );
    out.auth = require_auth(route, "google-vertex-anthropic ADC access token")?;
    Ok(out)
}

/// Bedrock region: option `region`, then `AWS_REGION`, then `us-east-1`.
pub fn bedrock_region(route: &CatalogRoute) -> String {
    route
        .option("region")
        .map(str::to_owned)
        .or_else(|| route.env_value("AWS_REGION"))
        .unwrap_or_else(|| bedrock_facade::DEFAULT_REGION.into())
}

/// Bearer token precedence: `AWS_BEARER_TOKEN_BEDROCK`, then option/credential key.
fn bedrock_bearer(route: &CatalogRoute) -> Option<String> {
    route
        .env_value("AWS_BEARER_TOKEN_BEDROCK")
        .or_else(|| route.option("bearerToken").map(str::to_owned))
        .or_else(|| route.api_key.clone())
}

fn env_credentials(route: &CatalogRoute, region: &str) -> Option<BedrockCredentials> {
    Some(BedrockCredentials {
        region: region.to_owned(),
        access_key_id: route.env_value("AWS_ACCESS_KEY_ID")?,
        secret_access_key: Secret::new(route.env_value("AWS_SECRET_ACCESS_KEY")?),
        session_token: route.env_value("AWS_SESSION_TOKEN").map(Secret::new),
    })
}

/// `@ai-sdk/amazon-bedrock`: Converse with region-prefixed model id. Returns the route and the
/// wire model id.
pub fn bedrock_converse(
    route: &CatalogRoute,
    transports: &Transports,
) -> Result<(ProtocolRoute, String), LlmError> {
    let region = bedrock_region(route);
    let base_url = route
        .option("endpoint")
        .map(str::to_owned)
        .or_else(|| route.url().map(str::to_owned));
    let credentials = env_credentials(route, &region);
    let auth = match (bedrock_bearer(route), &credentials, &route.auth) {
        (Some(token), _, _) => Some(bearer_value(token)),
        (None, Some(_), _) => None,
        (None, None, Some(auth)) => Some(auth.clone()),
        (None, None, None) => {
            return Err(unported(
                route,
                "amazon-bedrock AWS credential chain (profile/SSO/web identity/container)",
            ));
        }
    };
    let config = ProviderConfig {
        auth,
        base_url,
        env: route.env.clone(),
        ..ProviderConfig::default()
    };
    let bedrock = BedrockConfig {
        credentials,
        region: Some(region.clone()),
    };
    let provider = bedrock_facade::configure(&config, &bedrock, transports);
    Ok((
        provider.route,
        resolve_model_id(&route.model_id, Some(&region)),
    ))
}

/// `@ai-sdk/amazon-bedrock/mantle`: OpenAI Responses/Chat at the Mantle URL, bearer only.
pub fn bedrock_mantle(
    route: &CatalogRoute,
    transports: &Transports,
) -> Result<ProtocolRoute, LlmError> {
    let Some(token) = bedrock_bearer(route) else {
        return Err(unported(route, "amazon-bedrock/mantle SigV4 signing"));
    };
    let mut out = if mantle_uses_chat(&route.model_id) {
        openai::chat_route(transports.sse.clone())
    } else {
        openai::responses_route(transports.sse.clone())
    };
    out.endpoint.base_url = Some(
        route
            .option("endpoint")
            .or(route.url())
            .map(str::to_owned)
            .unwrap_or_else(|| mantle_base_url(&bedrock_region(route))),
    );
    out.auth = bearer_value(token);
    Ok(out)
}

static EMPTY_ROLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""role"\s*:\s*"""#).expect("static regex"));

/// `cortexFetch` request/stream quirks as a protocol decorator.
pub fn snowflake_protocol() -> Decorated {
    Decorated {
        id: "snowflake-cortex-chat",
        inner: Arc::new(openai::OpenAIChatProtocol),
        body: Some(Arc::new(|_, body| {
            let Value::Object(mut body) = body else {
                return Ok(body);
            };
            if let Some(max) = body.shift_remove("max_tokens") {
                body.insert("max_completion_tokens".into(), max);
            }
            Ok(Value::Object(body))
        })),
        frame: Some(Arc::new(|text| {
            EMPTY_ROLE
                .replace_all(&text, r#""role":"assistant""#)
                .into_owned()
        })),
    }
}

/// Snowflake Cortex over OpenAI-compatible Chat at the account URL.
pub fn snowflake(route: &CatalogRoute, transports: &Transports) -> Result<ProtocolRoute, LlmError> {
    let (base, token) = loaders::snowflake(route)?;
    let mut out = ProtocolRoute::new(
        "snowflake-cortex",
        Arc::new(snowflake_protocol()),
        Endpoint::new(Some(&base), openai::CHAT_PATH),
        transports.sse.clone(),
    );
    out.auth = bearer_value(token);
    Ok(out)
}
