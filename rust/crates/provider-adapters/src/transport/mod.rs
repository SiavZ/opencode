//! Purpose: HTTP request executor (status mapping, redaction, bounded retries), SSE and AWS
//! event-stream framing, the `http-json` transport and the `websocket-json` transport.
//! Scope: module root re-exporting the transport building blocks.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/transport/event_stream.rs
//! |-- provider-adapters/src/transport/executor.rs
//! |-- provider-adapters/src/transport/http.rs
//! |-- provider-adapters/src/transport/overflow.rs
//! |-- provider-adapters/src/transport/redact.rs
//! |-- provider-adapters/src/transport/sse.rs
//! |-- provider-adapters/src/transport/status.rs
//! `-- provider-adapters/src/transport/websocket.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/lib.rs
//! |-- provider-adapters/src/protocols/anthropic/stream.rs
//! |-- provider-adapters/src/protocols/bedrock/stream.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/error.rs
//! |-- provider-adapters/tests/fixtures/anthropic-support/cassette.rs
//! |-- provider-adapters/tests/openai_chat.rs
//! |-- provider-adapters/tests/openai_responses.rs
//! |-- provider-adapters/tests/providers_support/mod.rs
//! |-- provider-adapters/tests/transport_executor.rs
//! |-- provider-adapters/tests/transport_http.rs
//! |-- provider-adapters/tests/transport_websocket.rs
//! `-- runtime/src/location_routes.rs
//! Invariants: owned by phase-03 leaf p03.transport; other leaves do not edit files under this
//! directory. Transports implement `route::Transport` and never spawn detached tasks.
//! Contract tests: provider-adapters/tests/transport_executor.rs, provider-adapters/tests/transport_http.rs, provider-adapters/tests/transport_framing.rs, provider-adapters/tests/transport_websocket.rs

pub mod event_stream;
pub mod executor;
pub mod http;
pub mod overflow;
pub mod redact;
pub mod sse;
pub mod status;
pub mod websocket;

pub use executor::{HttpExecutor, MAX_RETRIES};
pub use http::{Framing, HttpJsonTransport};
pub use overflow::is_context_overflow;
pub use websocket::WebSocketJsonTransport;
