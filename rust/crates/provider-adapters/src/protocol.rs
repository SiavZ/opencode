//! Purpose: the `Protocol` seam: one model-server family's body builder and streaming
//! state machine (packages/llm/src/route/protocol.ts). Protocols own no URL, header or auth.
//! Scope: dyn-compatible trait plus the `Frame` type shared by framings and decoders.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/lib.rs
//! `-- provider-adapters/src/route.rs
//! Invariants: `step` is pure over (state, frame) and emits events in provider order.
//! `on_halt` runs once when the framed stream ends without a terminal frame.
//! Contract tests: provider-adapters/tests/route_http.rs

use opencode_llm_api::{LlmError, LlmEvent, LlmRequest};
use serde_json::Value;

/// One unit cut from the response body by a framing.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    /// SSE `data:` payload (already filtered for empty and `[DONE]`), or one WebSocket text message.
    Text(String),
    /// AWS event-stream record already projected to `{ "<event-type>": payload }`.
    Json(Value),
}

impl Frame {
    /// Parse the frame as JSON, mapping failures to `InvalidProviderOutput` for `route`.
    pub fn json(&self, route: &str, message: &str) -> Result<Value, LlmError> {
        match self {
            Frame::Json(value) => Ok(value.clone()),
            Frame::Text(text) => serde_json::from_str(text)
                .map_err(|_| LlmError::invalid_output(route, message, Some(text.clone()))),
        }
    }

    pub fn raw(&self) -> String {
        match self {
            Frame::Text(text) => text.clone(),
            Frame::Json(value) => value.to_string(),
        }
    }
}

/// Per-response parser state. Each protocol defines its own concrete state.
pub trait ProtocolState: Send {
    /// Translate one frame into emitted events. `route` is `"{provider}/{route_id}"`.
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError>;
    /// Whether the frame just stepped closes the response (transports that do not end, e.g. WebSocket).
    fn terminal(&self) -> bool {
        false
    }
    /// Flush emitted when the framed stream ends.
    fn on_halt(&mut self, _route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        Ok(Vec::new())
    }
}

/// The semantic API contract of one model-server family.
pub trait Protocol: Send + Sync + std::fmt::Debug {
    /// Stable protocol id, e.g. `openai-chat`, `anthropic-messages`.
    fn id(&self) -> &'static str;
    /// Build and validate the provider-native JSON body from an option-resolved request.
    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError>;
    /// Fresh parser state for one response.
    fn initial(&self, request: &LlmRequest) -> Box<dyn ProtocolState>;
}
