//! Purpose: consumer-facing client contract (`LLMClient`: prepare/stream) and `generate`,
//! which folds one streamed turn into a completed response.
//! Scope: trait plus a runtime-agnostic async helper.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/message.rs
//! |-- llm-api/src/response.rs
//! `-- llm-api/src/route.rs
//! External:
//! `-- futures::StreamExt
//! Used by:
//! `-- llm-api/src/lib.rs
//! Invariants: `generate` fails with `InvalidProviderOutput` ("Provider stream ended without
//! a terminal finish event") when the stream ends without `finish`/`provider-error`, and
//! surfaces the first stream error unchanged.
//! Contract tests: llm-api/tests/contract.rs

use futures::StreamExt;

use crate::error::LlmError;
use crate::message::LlmRequest;
use crate::response::{LlmResponse, ResponseState};
use crate::route::{LlmEventStream, PreparedRequest, resolve_request_options};

/// `LLMClient.Service`: what Session, title/compaction and hosted consumers call.
pub trait LlmClient: Send + Sync {
    fn prepare(&self, request: LlmRequest) -> Result<PreparedRequest, LlmError>;
    /// One provider turn. Dropping the stream cancels it.
    fn stream(&self, request: LlmRequest) -> LlmEventStream;
}

/// Minimal client that resolves option precedence and delegates to the request's route.
/// Provider-adapters wraps routes with cache-policy lowering before body construction.
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteClient;

impl LlmClient for RouteClient {
    fn prepare(&self, request: LlmRequest) -> Result<PreparedRequest, LlmError> {
        let request = resolve_request_options(request);
        request.model.route.prepare(&request)
    }

    fn stream(&self, request: LlmRequest) -> LlmEventStream {
        let request = resolve_request_options(request);
        let route = request.model.route.clone();
        route.stream(request)
    }
}

/// `LLM.generate`: run one turn to completion.
pub async fn generate(
    client: &dyn LlmClient,
    request: LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let label = request.model.route_label();
    let mut stream = client.stream(request);
    let mut state = ResponseState::default();
    while let Some(event) = stream.next().await {
        state.reduce(event?);
    }
    state.complete().ok_or_else(|| {
        LlmError::invalid_output(
            label,
            "Provider stream ended without a terminal finish event",
            None,
        )
    })
}
