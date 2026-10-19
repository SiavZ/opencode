//! Purpose: relay remote-workspace WebSocket frames with bounded backpressure and joined shutdown.
//! Scope: one accepted downstream connection and its upstream socket.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/services/streams.rs
//! `-- server/src/transport/websocket.rs
//! External:
//! |-- axum::extract::ws
//! |-- futures_util::{SinkExt, StreamExt}
//! |-- reqwest::Url
//! |-- tokio::time
//! `-- tokio_tungstenite
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/streams.rs
//! `-- server/src/handlers/streams/pty.rs
//! Invariants: no detached pump tasks or unbounded queues; shutdown closes both peers with 1001.
//! Contract tests: server/tests/ws_proxy.rs

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::app::AppState;
use crate::transport::websocket;

pub async fn upgrade(
    state: &AppState,
    mut target: reqwest::Url,
    headers: &HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(error) => {
            return crate::middleware::errors::HttpError::Defect(error.to_string()).into_response();
        }
    };
    let scheme = match target.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => scheme,
    }
    .to_string();
    if target.set_scheme(&scheme).is_err() {
        return crate::middleware::errors::HttpError::Defect(
            "Invalid WebSocket target scheme".into(),
        )
        .into_response();
    }
    // The TS socket constructor forwards protocols only, never inbound credentials or placement headers.
    let protocols = headers.get("sec-websocket-protocol").cloned();
    let offered = protocols
        .as_ref()
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let upgrade = upgrade.protocols(offered);
    let registration = state.services.streams.sockets.register();
    upgrade.on_upgrade(move |mut inbound| async move {
        let Some((_guard, closing)) = registration else {
            websocket::close_with(inbound, 1001, "server closing").await;
            return;
        };
        let mut request = match target.as_str().into_client_request() {
            Ok(request) => request,
            Err(_) => {
                websocket::close_with(inbound, 1011, "proxy error").await;
                return;
            }
        };
        if let Some(protocols) = protocols {
            request.headers_mut().insert("sec-websocket-protocol", protocols);
        }
        let connected = tokio::select! {
            _ = closing.cancelled() => {
                websocket::close_with(inbound, 1001, "server closing").await;
                return;
            }
            result = tokio_tungstenite::connect_async(request) => result,
        };
        let Ok((mut outbound, _)) = connected else {
            websocket::close_with(inbound, 1011, "proxy error").await;
            return;
        };
        loop {
            tokio::select! {
                _ = closing.cancelled() => {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                        let down = inbound.send(Message::Close(Some(CloseFrame {
                            code:1001, reason:"server closing".into(),
                        })));
                        let up = outbound.send(tungstenite::Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code:1001.into(), reason:"server closing".into(),
                        })));
                        let _ = tokio::join!(down, up);
                    }).await;
                    return;
                }
                frame = inbound.recv() => {
                    let Some(Ok(frame)) = frame else {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), outbound.close(None)).await;
                        return;
                    };
                    let close = matches!(frame, Message::Close(_));
                    let frame = upstream(frame);
                    let sent = tokio::select! {
                        _ = closing.cancelled() => continue,
                        result = outbound.send(frame) => result,
                    };
                    if close || sent.is_err() { return; }
                }
                frame = outbound.next() => {
                    let frame = match frame {
                        Some(Ok(frame)) => frame,
                        Some(Err(_)) => {
                            websocket::close_with(inbound, 1011, "proxy error").await;
                            return;
                        }
                        None => return,
                    };
                    let close = matches!(frame, tungstenite::Message::Close(_));
                    let Some(frame) = downstream(frame) else { continue; };
                    let sent = tokio::select! {
                        _ = closing.cancelled() => continue,
                        result = inbound.send(frame) => result,
                    };
                    if close || sent.is_err() { return; }
                }
            }
        }
    })
}

fn upstream(frame: Message) -> tungstenite::Message {
    match frame {
        Message::Text(text) => tungstenite::Message::Text(text.to_string().into()),
        Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
        Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
        Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
        Message::Close(frame) => tungstenite::Message::Close(frame.map(|frame| {
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        })),
    }
}

fn downstream(frame: tungstenite::Message) -> Option<Message> {
    Some(match frame {
        tungstenite::Message::Text(text) => Message::Text(text.to_string().into()),
        tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
        tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
        tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
        tungstenite::Message::Close(frame) => Message::Close(frame.map(|frame| CloseFrame {
            code: frame.code.into(),
            reason: frame.reason.to_string().into(),
        })),
        tungstenite::Message::Frame(_) => return None,
    })
}
