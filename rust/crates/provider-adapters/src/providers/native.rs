//! Purpose: Anthropic, Google (Gemini) and xAI facades (packages/llm/src/providers/anthropic.ts,
//! google.ts, xai.ts): header auth from `apiKey`/env, base URL override, xAI Responses default
//! with an OpenAI-compatible Chat alternative.
//! Scope: route configuration and model construction. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/src/providers/registry.rs
//! Invariants: Anthropic sends `x-api-key` plus `anthropic-version`; Google sends
//! `x-goog-api-key`; xAI sends bearer. Explicit `auth` overrides win.
//! Contract tests: provider-adapters/tests/providers_facades.rs

use std::sync::Arc;

use opencode_llm_api::{Model, ProviderId};

use crate::protocols::{anthropic as messages, gemini, openai};
use crate::providers::compatible;
use crate::providers::config::{ProviderConfig, Transports, apply_defaults, patch_endpoint};
use crate::route::ProtocolRoute;

pub const ANTHROPIC_ID: &str = "anthropic";
pub const ANTHROPIC_ENV: &[&str] = &["ANTHROPIC_API_KEY"];
pub const GOOGLE_ID: &str = "google";
pub const GOOGLE_ENV: &[&str] = &["GOOGLE_GENERATIVE_AI_API_KEY"];
pub const XAI_ID: &str = "xai";
pub const XAI_ENV: &[&str] = &["XAI_API_KEY"];

/// One configured route bound to a provider id.
#[derive(Clone, Debug)]
pub struct SingleRoute {
    pub id: ProviderId,
    pub route: ProtocolRoute,
}

impl SingleRoute {
    pub fn model(&self, model: &str) -> Model {
        Model::new(model, self.id.clone(), Arc::new(self.route.clone()))
    }
}

/// `Anthropic.configure(input)`.
pub fn anthropic(config: &ProviderConfig, transports: &Transports) -> SingleRoute {
    let mut route = messages::route(transports.sse.clone());
    route.auth = config.header_auth("x-api-key", ANTHROPIC_ENV);
    patch_endpoint(&mut route, config.base_url.as_deref(), &config.query);
    apply_defaults(&mut route, &config.defaults);
    SingleRoute {
        id: ProviderId::new(ANTHROPIC_ID),
        route,
    }
}

/// `Google.configure(input)`.
pub fn google(config: &ProviderConfig, transports: &Transports) -> SingleRoute {
    let mut route = gemini::route(transports.sse.clone());
    route.auth = config.header_auth("x-goog-api-key", GOOGLE_ENV);
    patch_endpoint(&mut route, config.base_url.as_deref(), &config.query);
    apply_defaults(&mut route, &config.defaults);
    SingleRoute {
        id: ProviderId::new(GOOGLE_ID),
        route,
    }
}

/// `XAI.configure(input)`: `model` is Responses, `chat` is OpenAI-compatible Chat.
#[derive(Clone, Debug)]
pub struct XaiProvider {
    pub responses: SingleRoute,
    pub chat: SingleRoute,
}

impl XaiProvider {
    pub fn model(&self, id: &str) -> Model {
        self.responses.model(id)
    }
}

pub fn xai(config: &ProviderConfig, transports: &Transports) -> XaiProvider {
    let base_url = config
        .base_url
        .clone()
        .or_else(|| compatible::profile(XAI_ID).map(|p| p.base_url.to_owned()));
    let configured = |mut route: ProtocolRoute| {
        route.provider = Some(ProviderId::new(XAI_ID));
        route.auth = config.bearer(XAI_ENV);
        patch_endpoint(&mut route, base_url.as_deref(), &config.query);
        apply_defaults(&mut route, &config.defaults);
        SingleRoute {
            id: ProviderId::new(XAI_ID),
            route,
        }
    };
    XaiProvider {
        responses: configured(openai::responses_route(transports.sse.clone())),
        chat: configured(openai::compatible_chat_route(transports.sse.clone())),
    }
}
