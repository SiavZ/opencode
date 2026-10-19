//! Purpose: OpenAI-compatible Chat facade and family profiles (packages/llm/src/providers/
//! openai-compatible.ts, openai-compatible-profile.ts): generic `configure` with required
//! `baseURL`, bearer auth with no env fallback, and the baseten..togetherai profile defaults.
//! Scope: route configuration and model construction. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/config.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! `-- openai::compatible_chat_route
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/providers/native.rs
//! `-- provider-adapters/tests/providers_facades.rs
//! Invariants: the route id stays `openai-compatible-chat`; the provider defaults to
//! `openai-compatible`. Profiles only change provider id and default base URL.
//! Contract tests: provider-adapters/tests/providers_facades.rs

use std::sync::Arc;

use opencode_llm_api::{LlmError, Model, ProviderId};

use crate::protocols::openai;
use crate::providers::config::{
    ProviderConfig, Transports, apply_defaults, patch_endpoint, require_base_url,
};
use crate::route::ProtocolRoute;

pub const ID: &str = "openai-compatible";

/// `OpenAICompatibleProfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub provider: &'static str,
    pub base_url: &'static str,
}

/// `profiles`, in baseline declaration order.
pub const PROFILES: &[Profile] = &[
    Profile {
        provider: "baseten",
        base_url: "https://inference.baseten.co/v1",
    },
    Profile {
        provider: "cerebras",
        base_url: "https://api.cerebras.ai/v1",
    },
    Profile {
        provider: "deepinfra",
        base_url: "https://api.deepinfra.com/v1/openai",
    },
    Profile {
        provider: "deepseek",
        base_url: "https://api.deepseek.com/v1",
    },
    Profile {
        provider: "fireworks",
        base_url: "https://api.fireworks.ai/inference/v1",
    },
    Profile {
        provider: "groq",
        base_url: "https://api.groq.com/openai/v1",
    },
    Profile {
        provider: "openrouter",
        base_url: "https://openrouter.ai/api/v1",
    },
    Profile {
        provider: "togetherai",
        base_url: "https://api.together.xyz/v1",
    },
    Profile {
        provider: "xai",
        base_url: "https://api.x.ai/v1",
    },
];

/// `byProvider[provider]`.
pub fn profile(provider: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.provider == provider)
}

/// A configured OpenAI-compatible Chat facade.
#[derive(Clone, Debug)]
pub struct CompatibleProvider {
    pub id: ProviderId,
    pub route: ProtocolRoute,
}

/// `OpenAICompatible.configure({ provider?, baseURL, ... })`. `env_vars` extends the baseline
/// (which passes `[]`) for registry callers whose SDK reads an env key (e.g. `GROQ_API_KEY`).
pub fn configure_with_env(
    provider: Option<&str>,
    config: &ProviderConfig,
    env_vars: &[&str],
    transports: &Transports,
) -> Result<CompatibleProvider, LlmError> {
    let built = build(provider.unwrap_or(ID), config, env_vars, transports);
    require_base_url(&built.route)?;
    Ok(built)
}

fn build(
    provider: &str,
    config: &ProviderConfig,
    env_vars: &[&str],
    transports: &Transports,
) -> CompatibleProvider {
    let mut route = openai::compatible_chat_route(transports.sse.clone());
    route.provider = Some(ProviderId::new(provider));
    route.auth = config.bearer(env_vars);
    patch_endpoint(&mut route, config.base_url.as_deref(), &config.query);
    apply_defaults(&mut route, &config.defaults);
    CompatibleProvider {
        id: ProviderId::new(provider),
        route,
    }
}

/// `OpenAICompatible.configure(input)`: bearer from `apiKey` only.
pub fn configure(
    provider: Option<&str>,
    config: &ProviderConfig,
    transports: &Transports,
) -> Result<CompatibleProvider, LlmError> {
    configure_with_env(provider, config, &[], transports)
}

/// `define(profile)(input)`: profile base URL unless overridden.
pub fn configure_profile(
    profile: &Profile,
    config: &ProviderConfig,
    transports: &Transports,
) -> CompatibleProvider {
    let mut config = config.clone();
    config.base_url = config
        .base_url
        .or_else(|| Some(profile.base_url.to_owned()));
    build(profile.provider, &config, &[], transports)
}

impl CompatibleProvider {
    pub fn model(&self, id: &str) -> Model {
        Model::new(id, self.id.clone(), Arc::new(self.route.clone()))
    }
}
