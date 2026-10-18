//! Purpose: OpenAI Chat Completions, OpenAI-compatible Chat and OpenAI Responses (HTTP + WebSocket)
//! protocol families and their default route constructors (packages/llm/src/protocols/
//! openai-chat.ts, openai-compatible-chat.ts, openai-responses.ts).
//! Scope: module root: route constructors and transport factories used by p03.providers.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/protocols/openai/chat_stream.rs
//! |-- provider-adapters/src/protocols/openai/options.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! |-- provider-adapters/src/protocols/openai/responses_events.rs
//! |-- provider-adapters/src/protocols/openai/responses_stream.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/mod.rs
//! Invariants: owned by phase-03 leaf p03.openai. Routes start with `NoAuth`; providers attach
//! auth, base URL overrides and headers. Responses routes default `providerOptions.openai.store`
//! to `false`. The WebSocket route shares the Responses protocol and differs only in transport.
//! Contract tests: provider-adapters/tests/openai_chat.rs, provider-adapters/tests/openai_responses.rs, provider-adapters/tests/openai_recorded.rs

pub mod chat;
pub mod chat_stream;
pub mod options;
pub mod responses_body;
pub mod responses_events;
pub mod responses_stream;
pub mod wire;

use std::sync::Arc;

use opencode_llm_api::{JsonObject, ProviderId, RouteDefaults};
use serde_json::Value;

use crate::route::{Endpoint, ProtocolRoute, Transport};
use crate::transport::{HttpExecutor, HttpJsonTransport, WebSocketJsonTransport};

pub use chat::OpenAIChatProtocol;
pub use responses_stream::OpenAIResponsesProtocol;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const CHAT_PATH: &str = "/chat/completions";
pub const RESPONSES_PATH: &str = "/responses";
pub const CHAT_ROUTE_ID: &str = "openai-chat";
pub const COMPATIBLE_CHAT_ROUTE_ID: &str = "openai-compatible-chat";
pub const RESPONSES_ROUTE_ID: &str = "openai-responses";
pub const RESPONSES_WEBSOCKET_ROUTE_ID: &str = "openai-responses-websocket";

/// `HttpTransport.sseJson`: http-json with SSE framing.
pub fn sse_transport(executor: HttpExecutor) -> Arc<dyn Transport> {
    Arc::new(HttpJsonTransport::sse(executor))
}

/// `OpenAIResponses.webSocketTransport`: websocket-json sending `response.create`.
pub fn web_socket_transport() -> Arc<dyn Transport> {
    Arc::new(WebSocketJsonTransport::new(Arc::new(
        responses_body::web_socket_message,
    )))
}

fn store_false_defaults() -> RouteDefaults {
    let mut openai = JsonObject::new();
    openai.insert("store".into(), Value::Bool(false));
    RouteDefaults {
        provider_options: Some([("openai".to_owned(), openai)].into_iter().collect()),
        ..RouteDefaults::default()
    }
}

/// `OpenAIChat.route`: provider `openai`, `https://api.openai.com/v1/chat/completions`.
pub fn chat_route(transport: Arc<dyn Transport>) -> ProtocolRoute {
    let mut route = ProtocolRoute::new(
        CHAT_ROUTE_ID,
        Arc::new(OpenAIChatProtocol),
        Endpoint::new(Some(DEFAULT_BASE_URL), CHAT_PATH),
        transport,
    );
    route.provider = Some(ProviderId::new("openai"));
    route
}

/// `OpenAICompatibleChat.route`: the Chat protocol with no default base URL or provider.
/// Providers set `endpoint.base_url` before use.
pub fn compatible_chat_route(transport: Arc<dyn Transport>) -> ProtocolRoute {
    ProtocolRoute::new(
        COMPATIBLE_CHAT_ROUTE_ID,
        Arc::new(OpenAIChatProtocol),
        Endpoint::new(None, CHAT_PATH),
        transport,
    )
}

/// `OpenAIResponses.route`: provider `openai`, `.../v1/responses`, `store: false` default.
pub fn responses_route(transport: Arc<dyn Transport>) -> ProtocolRoute {
    let mut route = ProtocolRoute::new(
        RESPONSES_ROUTE_ID,
        Arc::new(OpenAIResponsesProtocol),
        Endpoint::new(Some(DEFAULT_BASE_URL), RESPONSES_PATH),
        transport,
    );
    route.provider = Some(ProviderId::new("openai"));
    route.defaults = store_false_defaults();
    route
}

/// `OpenAIResponses.webSocketRoute`: same protocol and endpoint over `websocket-json`.
pub fn responses_websocket_route() -> ProtocolRoute {
    let mut route = responses_route(web_socket_transport());
    route.id = RESPONSES_WEBSOCKET_ROUTE_ID.to_owned();
    route
}
