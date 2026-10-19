//! Purpose: `websocket-json` transport (packages/llm/src/route/transport/websocket.ts
//! `WebSocketExecutor` + `WebSocketTransport.json`) used by OpenAI Responses WebSocket mode:
//! convert the endpoint to ws(s), open with the prepared headers, send one JSON message built
//! from the request body, and yield every incoming message as `Frame::Text`.
//! Scope: one socket per `frames` call over tokio-tungstenite (rustls, native roots).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! |-- async_stream::try_stream
//! |-- fmt::Debug
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- futures::SinkExt
//! |-- futures::StreamExt
//! |-- serde_json::Value
//! |-- str::to_owned
//! |-- tokio::net::TcpStream
//! |-- tokio_tungstenite::MaybeTlsStream
//! |-- tokio_tungstenite::WebSocketStream
//! |-- tokio_tungstenite::connect_async_with_config
//! |-- tokio_tungstenite::tungstenite::Message
//! |-- tokio_tungstenite::tungstenite::client::IntoClientRequest
//! |-- tokio_tungstenite::tungstenite::http::HeaderName
//! |-- tokio_tungstenite::tungstenite::http::HeaderValue
//! |-- tokio_tungstenite::tungstenite::protocol::WebSocketConfig
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/transport/mod.rs
//! Invariants: errors use module `WebSocketExecutor` with `Transport` reasons and kinds
//! `websocket` (URL), `open`, `write`, `message`, `close`, matching the baseline messages. Close
//! codes 1000 and 1005 (no status) end the stream cleanly, any other code fails it. Binary
//! messages are decoded as lossy UTF-8. Ping/pong are handled by tungstenite and never surface.
//! Reads are pulled on demand (no unbounded queue); one message is bounded by
//! `max_message_bytes`. Dropping the stream drops the socket (no detached close task).
//! Contract tests: provider-adapters/tests/transport_websocket.rs

use std::fmt;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use opencode_llm_api::{LlmError, LlmErrorReason};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::protocol::Frame;
use crate::route::{FrameStream, HttpRequestParts, Transport};

/// Default bound on one incoming WebSocket message (16 MiB).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// `toMessage` + `encodeMessage`: build the text message sent after open from the overlaid body.
pub type MessageFn = Arc<dyn Fn(Value) -> Result<String, LlmError> + Send + Sync>;

/// `WebSocketTransport.json({ toMessage, encodeMessage })`.
#[derive(Clone)]
pub struct WebSocketJsonTransport {
    to_message: MessageFn,
    max_message_bytes: usize,
}

impl fmt::Debug for WebSocketJsonTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketJsonTransport")
            .field("max_message_bytes", &self.max_message_bytes)
            .finish_non_exhaustive()
    }
}

impl WebSocketJsonTransport {
    pub fn new(to_message: MessageFn) -> Self {
        Self {
            to_message,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }

    pub fn max_message_bytes(mut self, bytes: usize) -> Self {
        self.max_message_bytes = bytes;
        self
    }
}

pub fn transport_error(
    method: &str,
    message: impl Into<String>,
    url: Option<&str>,
    kind: &str,
) -> LlmError {
    LlmError::new(
        "WebSocketExecutor",
        method,
        LlmErrorReason::Transport {
            message: message.into(),
            kind: Some(kind.to_owned()),
            url: url.map(str::to_owned),
            http: None,
        },
    )
}

/// `webSocketUrl`: `https:` -> `wss:`, `http:` -> `ws:`, anything else fails.
pub fn web_socket_url(value: &str) -> Result<String, LlmError> {
    let fail = |message: String| transport_error("prepare", message, Some(value), "websocket");
    let mut url = url::Url::parse(value).map_err(|_| fail("Invalid URL".into()))?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => return Err(fail(format!("Unsupported WebSocket URL protocol {other}:"))),
    };
    url.set_scheme(scheme).map_err(|_| {
        fail(format!(
            "Unsupported WebSocket URL protocol {}:",
            url.scheme()
        ))
    })?;
    Ok(url.to_string())
}

impl Transport for WebSocketJsonTransport {
    fn id(&self) -> &str {
        "websocket-json"
    }

    fn frames(&self, request: HttpRequestParts, _route_label: String) -> FrameStream {
        let to_message = self.to_message.clone();
        let max_message_bytes = self.max_message_bytes;
        let stream = async_stream::try_stream! {
            let url = web_socket_url(&request.url)?;
            let message = to_message(request.json_body.clone())?;
            let mut socket = open(&url, &request, max_message_bytes).await?;
            socket
                .send(Message::text(message))
                .await
                .map_err(|e| transport_error("sendText", e.to_string(), Some(&url), "write"))?;
            while let Some(item) = socket.next().await {
                let item = item.map_err(|e| {
                    transport_error("message", format!("WebSocket error: {e}"), Some(&url), "message")
                })?;
                match item {
                    Message::Text(text) => yield Frame::Text(text.as_str().to_owned()),
                    Message::Binary(bytes) => {
                        yield Frame::Text(String::from_utf8_lossy(&bytes).into_owned())
                    }
                    Message::Close(frame) => {
                        let code = frame.map_or(1005, |f| u16::from(f.code));
                        if code != 1000 && code != 1005 {
                            Err(transport_error(
                                "message",
                                format!("WebSocket closed with code {code}"),
                                Some(&url),
                                "close",
                            ))?;
                        }
                        break;
                    }
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
        };
        Box::pin(stream)
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(
    url: &str,
    request: &HttpRequestParts,
    max_message_bytes: usize,
) -> Result<Socket, LlmError> {
    let fail = |message: String| transport_error("open", message, Some(url), "open");
    let mut handshake = url
        .into_client_request()
        .map_err(|e| fail(format!("Failed to open WebSocket: {e}")))?;
    for (name, value) in &request.headers {
        // The body is sent as a WebSocket message, never as an HTTP entity.
        if name.eq_ignore_ascii_case("content-type") || name.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| fail(format!("Failed to construct WebSocket: {e}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|e| fail(format!("Failed to construct WebSocket: {e}")))?;
        handshake.headers_mut().append(name, value);
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(max_message_bytes))
        .max_frame_size(Some(max_message_bytes));
    let (socket, _) = tokio_tungstenite::connect_async_with_config(handshake, Some(config), true)
        .await
        .map_err(|e| fail(format!("Failed to open WebSocket: {e}")))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_conversion() {
        assert_eq!(
            web_socket_url("https://api.openai.com/v1/responses?x=1").unwrap(),
            "wss://api.openai.com/v1/responses?x=1"
        );
        assert_eq!(
            web_socket_url("http://127.0.0.1:9/v1").unwrap(),
            "ws://127.0.0.1:9/v1"
        );
        let err = web_socket_url("wss://x.test/v1").unwrap_err();
        assert_eq!(
            err.reason.message(),
            "Unsupported WebSocket URL protocol wss:"
        );
    }
}
