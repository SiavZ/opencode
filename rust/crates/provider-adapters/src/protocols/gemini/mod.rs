//! Purpose: Google Gemini generateContent protocol (packages/llm/src/protocols/gemini.ts): the
//! `Protocol` impl and the default route constructor (generativelanguage.googleapis.com/v1beta,
//! `/models/{model}:streamGenerateContent?alt=sse`, SSE).
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/gemini/body.rs
//! |-- provider-adapters/src/protocols/gemini/stream.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/mod.rs
//! `-- provider-adapters/tests/gemini.rs
//! Invariants: owned by phase-03 leaf p03.messages; the path embeds the model id and pins SSE
//! via `alt=sse`; auth (`x-goog-api-key`) is supplied by the provider registration.
//! Contract tests: provider-adapters/tests/gemini.rs

mod body;
mod stream;

use std::sync::Arc;

use opencode_llm_api::{LlmError, LlmRequest, ProviderId};
use serde_json::Value;

use crate::protocol::{Protocol, ProtocolState};
use crate::route::{Endpoint, EndpointPath, ProtocolRoute, Transport};

pub const ID: &str = "gemini";
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// The Gemini protocol: body lowering plus the SSE candidate state machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gemini;

impl Protocol for Gemini {
    fn id(&self) -> &'static str {
        ID
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        body::body(request)
    }

    fn initial(&self, _request: &LlmRequest) -> Box<dyn ProtocolState> {
        Box::new(stream::State::default())
    }
}

/// `/models/{model}:streamGenerateContent?alt=sse`.
pub fn path(model: &str) -> String {
    format!("/models/{model}:streamGenerateContent?alt=sse")
}

/// `Gemini.route`: provider `google`, `Auth.none`, SSE framing expected from `transport`.
pub fn route(transport: Arc<dyn Transport>) -> ProtocolRoute {
    let mut route = ProtocolRoute::new(
        ID,
        Arc::new(Gemini),
        Endpoint {
            base_url: Some(DEFAULT_BASE_URL.to_owned()),
            path: EndpointPath::Dynamic(Arc::new(|request, _| path(request.model.id.as_str()))),
            query: Default::default(),
        },
        transport,
    );
    route.provider = Some(ProviderId::new("google"));
    route
}
