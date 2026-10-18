//! Purpose: `http-json` transport (packages/llm/src/route/transport/http.ts `httpJson`, framing.ts):
//! execute the prepared POST through `HttpExecutor`, then cut the response body into protocol
//! frames with the selected framing (SSE or AWS event-stream).
//! Scope: response byte streaming and framing selection. Request preparation is route.rs.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/event_stream.rs
//! |-- provider-adapters/src/transport/executor.rs
//! `-- provider-adapters/src/transport/sse.rs
//! External:
//! |-- async_stream::try_stream
//! `-- futures::StreamExt
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/tests/transport_http.rs
//! Invariants: retries only happen inside `execute`, before the first body byte; once a 2xx
//! response is streaming, read and framing failures end the stream with
//! `InvalidProviderOutput` and are never retried. Body read errors map to
//! `Failed to read <route> stream`. The stream owns the response: dropping it aborts the request.
//! Contract tests: provider-adapters/tests/transport_http.rs

use futures::StreamExt;
use opencode_llm_api::LlmError;

use crate::protocol::Frame;
use crate::route::{FrameStream, HttpRequestParts, Transport};
use crate::transport::event_stream::{DEFAULT_MAX_FRAME_BYTES, EventStreamDecoder};
use crate::transport::executor::HttpExecutor;
use crate::transport::sse::{DEFAULT_MAX_EVENT_BYTES, SseParser};

/// Response body framing (`Framing`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Framing {
    /// `Framing.sse`: one `Frame::Text` per non-empty, non-`[DONE]` event.
    Sse { max_event_bytes: usize },
    /// `BedrockEventStream.framing(route)`: one `Frame::Json` per event record. `route`
    /// labels decode errors (the baseline uses the adapter id, `bedrock-converse`).
    AwsEventStream {
        route: String,
        max_frame_bytes: usize,
    },
}

impl Framing {
    pub fn sse() -> Self {
        Self::Sse {
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
        }
    }

    pub fn aws_event_stream(route: impl Into<String>) -> Self {
        Self::AwsEventStream {
            route: route.into(),
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::Sse { .. } => "sse",
            Self::AwsEventStream { .. } => "aws-event-stream",
        }
    }
}

/// Stateful framer for one response.
enum Framer {
    Sse(SseParser),
    Aws(EventStreamDecoder),
}

impl Framer {
    fn new(framing: &Framing) -> Self {
        match framing {
            Framing::Sse { max_event_bytes } => Self::Sse(SseParser::new(*max_event_bytes)),
            Framing::AwsEventStream {
                route,
                max_frame_bytes,
            } => Self::Aws(EventStreamDecoder::with_limit(
                route.clone(),
                *max_frame_bytes,
            )),
        }
    }

    /// Returns frames plus whether the stream must stop (SSE `retry:`).
    fn feed(&mut self, chunk: &[u8], route_label: &str) -> Result<(Vec<Frame>, bool), LlmError> {
        match self {
            Self::Sse(parser) => {
                let frames = parser.feed(chunk).map_err(|e| {
                    LlmError::invalid_output(
                        route_label,
                        format!("Failed to read {route_label} stream"),
                        Some(e.to_string()),
                    )
                })?;
                Ok((
                    frames.into_iter().map(Frame::Text).collect(),
                    parser.ended(),
                ))
            }
            Self::Aws(decoder) => Ok((
                decoder.feed(chunk)?.into_iter().map(Frame::Json).collect(),
                false,
            )),
        }
    }
}

/// `HttpTransport.httpJson({ framing })`.
#[derive(Clone, Debug)]
pub struct HttpJsonTransport {
    executor: HttpExecutor,
    framing: Framing,
}

impl HttpJsonTransport {
    pub fn new(executor: HttpExecutor, framing: Framing) -> Self {
        Self { executor, framing }
    }

    /// `sseJson`: http-json with SSE framing.
    pub fn sse(executor: HttpExecutor) -> Self {
        Self::new(executor, Framing::sse())
    }

    /// `.with({ framing })`.
    pub fn with_framing(&self, framing: Framing) -> Self {
        Self::new(self.executor.clone(), framing)
    }

    pub fn framing(&self) -> &Framing {
        &self.framing
    }
}

impl Transport for HttpJsonTransport {
    fn id(&self) -> &str {
        "http-json"
    }

    fn frames(&self, request: HttpRequestParts, route_label: String) -> FrameStream {
        let executor = self.executor.clone();
        let mut framer = Framer::new(&self.framing);
        let stream = async_stream::try_stream! {
            let response = executor.execute(&request).await?;
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| {
                    LlmError::invalid_output(
                        &route_label,
                        format!("Failed to read {route_label} stream"),
                        Some(crate::transport::executor::error_text(&e)),
                    )
                })?;
                let (frames, ended) = framer.feed(&chunk, &route_label)?;
                for frame in frames {
                    yield frame;
                }
                if ended {
                    break;
                }
            }
        };
        Box::pin(stream)
    }
}
