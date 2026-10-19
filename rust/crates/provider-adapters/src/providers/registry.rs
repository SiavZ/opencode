//! Purpose: the provider registry: resolve a catalog provider/model (`CatalogRoute`) into a
//! runnable `opencode_llm_api::Model` by dispatching on provider id and AI SDK package exactly
//! where the baseline plugins do (`ctx.aisdk.sdk` + `ctx.aisdk.language` hooks of
//! packages/core/src/plugin/provider/*.ts, legacy `custom()` loaders), or fail with the typed
//! `NoRoute (not yet ported)` error.
//! Scope: dispatch and per-package route rules (base URL, env key, headers, route family).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/unsupported.rs
//! External:
//! |-- cloud::bedrock_converse
//! |-- cloud::bedrock_mantle
//! |-- cloud::snowflake
//! |-- cloud::vertex_anthropic
//! |-- cloud::vertex_gemini
//! |-- gateways::openrouter
//! |-- gemini::route
//! |-- loaders::catalog_headers
//! |-- loaders::opencode_public_key
//! |-- messages::route
//! |-- openai::compatible_chat_route
//! |-- openai::responses_route
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/src/providers/registry_rules.rs
//! Invariants: every `PLUGINS` id resolves or fails typed; unknown packages are the
//! dynamic-provider path and fail typed. Header precedence: route defaults < catalog headers.
//! Contract tests: provider-adapters/tests/providers_registry.rs

use std::sync::Arc;

use opencode_llm_api::{LlmError, Model, ModelId, ProviderId, RouteDefaults};

use crate::auth::SharedAuth;
use crate::protocols::{anthropic as messages, gemini, openai};
use crate::providers::cloud;
use crate::providers::config::{ProviderConfig, Transports, apply_defaults};
use crate::providers::gateways;
use crate::providers::inventory::{Api, CatalogRoute};
use crate::providers::loaders;
use crate::route::ProtocolRoute;
use crate::unsupported::not_yet_ported;

/// Default base URL and env key for AI SDK packages that speak OpenAI Chat Completions.
const CHAT_PACKAGES: &[(&str, &str, &str)] = &[
    (
        "@ai-sdk/alibaba",
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        "ALIBABA_API_KEY",
    ),
    (
        "@ai-sdk/cerebras",
        "https://api.cerebras.ai/v1",
        "CEREBRAS_API_KEY",
    ),
    (
        "@ai-sdk/togetherai",
        "https://api.together.xyz/v1",
        "TOGETHER_API_KEY",
    ),
    ("@ai-sdk/vercel", "https://api.v0.dev/v1", "VERCEL_API_KEY"),
    (
        "venice-ai-sdk-provider",
        "https://api.venice.ai/api/v1",
        "VENICE_API_KEY",
    ),
    (
        "@ai-sdk/groq",
        "https://api.groq.com/openai/v1",
        "GROQ_API_KEY",
    ),
    (
        "@ai-sdk/mistral",
        "https://api.mistral.ai/v1",
        "MISTRAL_API_KEY",
    ),
    (
        "@ai-sdk/perplexity",
        "https://api.perplexity.ai",
        "PERPLEXITY_API_KEY",
    ),
];

/// Packages with their own wire protocol and no Rust port yet.
const UNPORTED_PACKAGES: &[(&str, &str)] = &[
    ("@ai-sdk/cohere", "cohere v2 chat protocol"),
    ("@ai-sdk/gateway", "vercel ai-gateway v3 protocol"),
    ("gitlab-ai-provider", "gitlab duo agentic/workflow chat"),
];

/// Resolves catalog models into runnable routes over shared transports.
#[derive(Clone, Debug)]
pub struct ProviderRegistry {
    pub transports: Transports,
    /// `opencode/<version>` used in Cloudflare user agents.
    pub user_agent: String,
    /// OS release for `(<platform> <release>; <arch>)` user agent suffixes.
    pub os_release: String,
}

impl ProviderRegistry {
    pub fn new(transports: Transports, version: &str) -> Self {
        Self {
            transports,
            user_agent: format!("opencode/{version}"),
            os_release: String::new(),
        }
    }

    /// Catalog transforms, then per-package route construction.
    pub fn resolve(&self, input: &CatalogRoute) -> Result<Model, LlmError> {
        let mut route = input.clone();
        loaders::catalog_headers(&mut route);
        loaders::opencode_public_key(&mut route);
        let (mut protocol_route, model_id) = self.route(&route)?;
        protocol_route.provider = Some(ProviderId::new(route.provider_id.as_str()));
        let defaults = RouteDefaults {
            headers: (!route.headers.is_empty()).then(|| route.headers.clone()),
            ..RouteDefaults::default()
        };
        apply_defaults(&mut protocol_route, &defaults);
        Ok(Model::new(
            model_id,
            route.provider_id.as_str(),
            Arc::new(protocol_route),
        ))
    }

    fn route(&self, route: &CatalogRoute) -> Result<(ProtocolRoute, String), LlmError> {
        let unported = |what: &str| {
            Err(not_yet_ported(
                &ProviderId::new(route.provider_id.as_str()),
                &ModelId::new(route.model_id.as_str()),
                what,
            ))
        };
        let model = route.model_id.trim().to_owned();
        let package = match &route.api {
            Api::Native { .. } => return unported("native provider api"),
            Api::Aisdk { package, .. } => package.as_str(),
        };
        if route.provider_id == "sap-ai-core" {
            return unported("sap-ai-core dynamic SDK");
        }
        if route.provider_id == "snowflake-cortex" {
            return Ok((cloud::snowflake(route, &self.transports)?, model));
        }
        if let Some((_, what)) = UNPORTED_PACKAGES.iter().find(|(p, _)| *p == package) {
            return unported(what);
        }
        let sse = || self.transports.sse.clone();
        let one = |r: ProtocolRoute| Ok((r, model.clone()));
        match package {
            "@ai-sdk/openai" => {
                one(self.with_url(openai::responses_route(sse()), route, &["OPENAI_API_KEY"]))
            }
            "@ai-sdk/anthropic" => {
                let mut r = self.with_url(messages::route(sse()), route, &[]);
                r.auth = self.header_auth(route, "x-api-key", &["ANTHROPIC_API_KEY"]);
                one(r)
            }
            "@ai-sdk/google" => {
                let mut r = self.with_url(gemini::route(sse()), route, &[]);
                r.auth =
                    self.header_auth(route, "x-goog-api-key", &["GOOGLE_GENERATIVE_AI_API_KEY"]);
                one(r)
            }
            "@ai-sdk/xai" => {
                let mut r = openai::responses_route(sse());
                r.endpoint.base_url = Some(route.url().unwrap_or("https://api.x.ai/v1").to_owned());
                r.auth = self.bearer(route, &["XAI_API_KEY"]);
                one(r)
            }
            "@ai-sdk/deepinfra" => {
                let base = route
                    .url()
                    .unwrap_or("https://api.deepinfra.com/v1")
                    .trim_end_matches('/');
                one(self.chat(route, &format!("{base}/openai"), &["DEEPINFRA_API_KEY"]))
            }
            "@openrouter/ai-sdk-provider" => {
                let config = self.config(route);
                one(gateways::openrouter(&config, &self.transports).route)
            }
            "@ai-sdk/azure" => one(self.azure(route)?),
            "@ai-sdk/github-copilot" => one(self.copilot(route)),
            "ai-gateway-provider" => one(self.cloudflare_gateway(route)?),
            "@ai-sdk/amazon-bedrock" => cloud::bedrock_converse(route, &self.transports),
            "@ai-sdk/amazon-bedrock/mantle" => one(cloud::bedrock_mantle(route, &self.transports)?),
            "@ai-sdk/google-vertex" => one(cloud::vertex_gemini(route, &self.transports)?),
            "@ai-sdk/google-vertex/anthropic" => {
                one(cloud::vertex_anthropic(route, &self.transports)?)
            }
            p if p.contains("@ai-sdk/openai-compatible") => one(self.compatible(route)?),
            p => match CHAT_PACKAGES.iter().find(|(name, _, _)| *name == p) {
                Some((_, base, env)) => one(self.chat(route, route.url().unwrap_or(base), &[env])),
                None => unported("dynamic-provider npm/file package"),
            },
        }
    }

    pub(super) fn config(&self, route: &CatalogRoute) -> ProviderConfig {
        ProviderConfig {
            api_key: route
                .api_key
                .clone()
                .or_else(|| route.option("apiKey").map(str::to_owned)),
            auth: route.auth.clone(),
            base_url: route.url().map(str::to_owned),
            env: route.env.clone(),
            ..ProviderConfig::default()
        }
    }

    pub(super) fn env_names<'a>(route: &'a CatalogRoute, sdk_env: &[&'a str]) -> Vec<&'a str> {
        let mut names: Vec<&str> = route.env_vars.iter().map(String::as_str).collect();
        for env in sdk_env {
            if !names.contains(env) {
                names.push(env);
            }
        }
        names
    }

    pub(super) fn bearer(&self, route: &CatalogRoute, sdk_env: &[&str]) -> SharedAuth {
        self.config(route).bearer(&Self::env_names(route, sdk_env))
    }

    pub(super) fn header_auth(
        &self,
        route: &CatalogRoute,
        name: &str,
        sdk_env: &[&str],
    ) -> SharedAuth {
        self.config(route)
            .header_auth(name, &Self::env_names(route, sdk_env))
    }

    pub(super) fn with_url(
        &self,
        mut r: ProtocolRoute,
        route: &CatalogRoute,
        sdk_env: &[&str],
    ) -> ProtocolRoute {
        if let Some(url) = route.url() {
            r.endpoint.base_url = Some(url.to_owned());
        }
        r.auth = self.bearer(route, sdk_env);
        r
    }

    pub(super) fn chat(&self, route: &CatalogRoute, base: &str, sdk_env: &[&str]) -> ProtocolRoute {
        let mut r = openai::compatible_chat_route(self.transports.sse.clone());
        r.endpoint.base_url = Some(base.to_owned());
        r.auth = self.bearer(route, sdk_env);
        r
    }

    pub(super) fn ua(&self, product: &str) -> String {
        format!(
            "{} {product} {}",
            self.user_agent,
            loaders::platform_suffix(&self.os_release)
        )
    }
}
