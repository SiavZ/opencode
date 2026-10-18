//! Purpose: the static provider plugin inventory (packages/core/src/plugin/provider.ts:36-71) and
//! the catalog-driven input shape the registry resolves into runnable routes.
//! Scope: pure data: plugin ids, the AI SDK packages each one claims, route support status and
//! the `CatalogRoute` input (catalog provider/model fields plus credential).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! `-- provider-adapters/src/providers/config.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/src/providers/registry.rs
//! Invariants: `PLUGINS` lists every `ProviderPlugins` entry, in baseline order (34 ids).
//! Contract tests: provider-adapters/tests/providers_registry.rs

use opencode_llm_api::JsonObject;
use serde_json::Value;

use crate::auth::{Headers, SharedAuth};
use crate::providers::config::{Env, process_env};

/// How far a plugin's behavior is ported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Every claimed package resolves to a native Rust route.
    Native,
    /// Some modes resolve natively; others return the typed `NoRoute (not yet ported)` error.
    Partial,
    /// Needs an external runtime/SDK with no Rust protocol; always the typed error.
    NotYetPorted,
}

/// One `ProviderPlugins` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plugin {
    pub id: &'static str,
    /// AI SDK packages whose `aisdk.sdk` hook this plugin answers (empty: catalog/header only).
    pub packages: &'static [&'static str],
    pub support: Support,
}

const fn p(id: &'static str, packages: &'static [&'static str], support: Support) -> Plugin {
    Plugin {
        id,
        packages,
        support,
    }
}

use Support::{Native, NotYetPorted, Partial};

/// `ProviderPlugins`, in baseline order.
pub const PLUGINS: &[Plugin] = &[
    p("alibaba", &["@ai-sdk/alibaba"], Native),
    p(
        "amazon-bedrock",
        &["@ai-sdk/amazon-bedrock", "@ai-sdk/amazon-bedrock/mantle"],
        Partial,
    ),
    p("anthropic", &["@ai-sdk/anthropic"], Native),
    p("azure-cognitive-services", &[], Native),
    p("azure", &["@ai-sdk/azure"], Native),
    p("cerebras", &["@ai-sdk/cerebras"], Native),
    p("cloudflare-ai-gateway", &["ai-gateway-provider"], Native),
    p(
        "cloudflare-workers-ai",
        &["@ai-sdk/openai-compatible"],
        Native,
    ),
    p("cohere", &["@ai-sdk/cohere"], NotYetPorted),
    p("deepinfra", &["@ai-sdk/deepinfra"], Native),
    p("gateway", &["@ai-sdk/gateway"], NotYetPorted),
    p("github-copilot", &["@ai-sdk/github-copilot"], Native),
    p("gitlab", &["gitlab-ai-provider"], NotYetPorted),
    p("google", &["@ai-sdk/google"], Native),
    p(
        "google-vertex-anthropic",
        &["@ai-sdk/google-vertex/anthropic"],
        Partial,
    ),
    p("google-vertex", &["@ai-sdk/google-vertex"], Partial),
    p("groq", &["@ai-sdk/groq"], Partial),
    p("kilo", &[], Native),
    p("llmgateway", &[], Native),
    p("mistral", &["@ai-sdk/mistral"], Partial),
    p("nvidia", &[], Native),
    p("opencode", &[], Native),
    p("snowflake-cortex", &[], Partial),
    p("openai-compatible", &["@ai-sdk/openai-compatible"], Native),
    p("openai", &["@ai-sdk/openai"], Native),
    p("openrouter", &["@openrouter/ai-sdk-provider"], Native),
    p("perplexity", &["@ai-sdk/perplexity"], Partial),
    p("sap-ai-core", &[], NotYetPorted),
    p("togetherai", &["@ai-sdk/togetherai"], Native),
    p("vercel", &["@ai-sdk/vercel"], Native),
    p("venice", &["venice-ai-sdk-provider"], Native),
    p("xai", &["@ai-sdk/xai"], Native),
    p("zenmux", &[], Native),
    p("dynamic-provider", &["*"], NotYetPorted),
];

pub fn plugin(id: &str) -> Option<&'static Plugin> {
    PLUGINS.iter().find(|p| p.id == id)
}

/// Catalog `provider.api` (`ProviderV2.Api`).
#[derive(Clone, Debug, PartialEq)]
pub enum Api {
    Aisdk {
        package: String,
        url: Option<String>,
    },
    Native {
        url: Option<String>,
    },
}

/// Everything the registry needs to build one model route: the resolved catalog provider and
/// model (`provider.api`, `provider.request.{headers,body}`, `model.api.id`) and the active
/// credential. This is the `RouteFactory` input port shared with p03.catalog.
#[derive(Clone)]
pub struct CatalogRoute {
    pub provider_id: String,
    /// `model.api.id` (sent on the wire).
    pub model_id: String,
    pub api: Api,
    /// `provider.request.headers` merged with model/variant headers.
    pub headers: Headers,
    /// `provider.request.body` (AI SDK options: `resourceName`, `region`, `project`, ...).
    pub options: JsonObject,
    /// Stored credential key/access token, or `options.apiKey`.
    pub api_key: Option<String>,
    /// Credential metadata (`accountId`, `gatewayId`, `resourceName`, `account`).
    pub credential_metadata: JsonObject,
    /// Catalog `provider.env` names, consulted after `api_key`.
    pub env_vars: Vec<String>,
    /// Pre-built auth (e.g. Google ADC or AWS chain tokens from the credentials module).
    pub auth: Option<SharedAuth>,
    /// Whether an integration connection is active for this provider (llmgateway headers,
    /// opencode public key).
    pub connected: bool,
    pub env: Env,
}

impl CatalogRoute {
    pub fn new(provider_id: &str, model_id: &str, api: Api) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            api,
            headers: Headers::new(),
            options: JsonObject::new(),
            api_key: None,
            credential_metadata: JsonObject::new(),
            env_vars: Vec::new(),
            auth: None,
            connected: false,
            env: process_env(),
        }
    }

    pub fn aisdk(provider_id: &str, model_id: &str, package: &str, url: Option<&str>) -> Self {
        Self::new(
            provider_id,
            model_id,
            Api::Aisdk {
                package: package.to_owned(),
                url: url.map(str::to_owned),
            },
        )
    }

    pub fn package(&self) -> Option<&str> {
        match &self.api {
            Api::Aisdk { package, .. } => Some(package),
            Api::Native { .. } => None,
        }
    }

    pub fn url(&self) -> Option<&str> {
        match &self.api {
            Api::Aisdk { url, .. } | Api::Native { url } => {
                url.as_deref().filter(|u| !u.is_empty())
            }
        }
    }

    /// `typeof options[key] === "string"`.
    pub fn option(&self, key: &str) -> Option<&str> {
        match self.options.get(key) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn metadata(&self, key: &str) -> Option<&str> {
        match self.credential_metadata.get(key) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }

    /// Env lookup treating empty strings as unset (`env[...] ||`).
    pub fn env_value(&self, name: &str) -> Option<String> {
        (self.env)(name).filter(|v| !v.is_empty())
    }

    /// First set env var among `names`.
    pub fn first_env(&self, names: &[&str]) -> Option<String> {
        names.iter().find_map(|n| self.env_value(n))
    }
}
