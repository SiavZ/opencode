//! Purpose: Anthropic Messages protocol (packages/llm/src/protocols/anthropic-messages.ts):
//! the `Protocol` impl and the default route constructor (api.anthropic.com/v1/messages, SSE,
//! `anthropic-version: 2023-06-01`). Also hosts the wire helpers shared with Gemini and Bedrock.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/anthropic/body.rs
//! |-- provider-adapters/src/protocols/anthropic/stream.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/mod.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/tests/anthropic_messages.rs
//! `-- runtime/src/location_routes.rs
//! Invariants: owned by phase-03 leaf p03.messages; the protocol owns no URL, header or auth,
//! the route constructor supplies them; the transport is always caller-supplied.
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

mod body;
mod stream;
pub(crate) mod wire;

use std::sync::Arc;

use opencode_llm_api::{LlmError, LlmRequest, ProviderId};
use serde_json::Value;

use crate::auth::Headers;
use crate::protocol::{Protocol, ProtocolState};
use crate::route::{Endpoint, ProtocolRoute, Transport};

pub const ID: &str = "anthropic-messages";
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
pub const PATH: &str = "/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The Anthropic Messages protocol: body lowering plus the SSE event state machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnthropicMessages;

impl Protocol for AnthropicMessages {
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

/// `AnthropicMessages.route`: provider `anthropic`, SSE framing expected from `transport`,
/// `Auth.none` (callers add `x-api-key` auth), `anthropic-version` header.
pub fn route(transport: Arc<dyn Transport>) -> ProtocolRoute {
    let mut route = ProtocolRoute::new(
        ID,
        Arc::new(AnthropicMessages),
        Endpoint::new(Some(DEFAULT_BASE_URL), PATH),
        transport,
    );
    route.provider = Some(ProviderId::new("anthropic"));
    route.headers = Some(Arc::new(|_| {
        Headers::from([("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned())])
    }));
    route
}
