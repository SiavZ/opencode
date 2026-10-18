//! Purpose: protocol decorators for provider quirks that the baseline applies through custom
//! `fetch` wrappers or `transformRequestBody` hooks (OpenRouter body options, Vertex Anthropic
//! `anthropic_version`, Snowflake `max_completion_tokens` and `role:""` repair).
//! Scope: wraps an inner `Protocol`; body post-processing and per-frame text rewriting.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- provider-adapters/src/protocol.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/openrouter.rs
//! |-- provider-adapters/src/providers/snowflake.rs
//! `-- provider-adapters/src/providers/vertex.rs
//! Invariants: the wrapped protocol's stream semantics are unchanged except for the frame
//! rewrite; the decorator id is stable and distinct from the inner id.
//! Contract tests: provider-adapters/tests/providers_gateways.rs

use std::fmt;
use std::sync::Arc;

use opencode_llm_api::{LlmError, LlmEvent, LlmRequest};
use serde_json::Value;

use crate::protocol::{Frame, Protocol, ProtocolState};

pub type BodyFn = Arc<dyn Fn(&LlmRequest, Value) -> Result<Value, LlmError> + Send + Sync>;
pub type FrameFn = Arc<dyn Fn(String) -> String + Send + Sync>;

/// A protocol with a post-processed body and optionally rewritten text frames.
#[derive(Clone)]
pub struct Decorated {
    pub id: &'static str,
    pub inner: Arc<dyn Protocol>,
    pub body: Option<BodyFn>,
    pub frame: Option<FrameFn>,
}

impl fmt::Debug for Decorated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decorated")
            .field("id", &self.id)
            .field("inner", &self.inner.id())
            .finish_non_exhaustive()
    }
}

impl Protocol for Decorated {
    fn id(&self) -> &'static str {
        self.id
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let body = self.inner.body(request)?;
        match &self.body {
            Some(f) => f(request, body),
            None => Ok(body),
        }
    }

    fn initial(&self, request: &LlmRequest) -> Box<dyn ProtocolState> {
        let inner = self.inner.initial(request);
        match &self.frame {
            Some(f) => Box::new(RewriteState {
                inner,
                rewrite: f.clone(),
            }),
            None => inner,
        }
    }
}

struct RewriteState {
    inner: Box<dyn ProtocolState>,
    rewrite: FrameFn,
}

impl ProtocolState for RewriteState {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let frame = match frame {
            Frame::Text(text) => Frame::Text((self.rewrite)(text)),
            other => other,
        };
        self.inner.step(route, frame)
    }

    fn terminal(&self) -> bool {
        self.inner.terminal()
    }

    fn on_halt(&mut self, route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        self.inner.on_halt(route)
    }
}
