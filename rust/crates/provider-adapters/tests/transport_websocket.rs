//! Purpose: contract tests for the `websocket-json` transport against a local tokio-tungstenite
//! server: handshake headers, the single sent message, message-to-frame mapping, close-code
//! semantics, open failures and URL conversion errors.
//! Scope: fixture-based (local WebSocket server), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! |-- futures::SinkExt
//! |-- futures::StreamExt
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio_tungstenite::accept_hdr_async
//! |-- tokio_tungstenite::tungstenite::Message
//! |-- tokio_tungstenite::tungstenite::handshake::server::Request
//! |-- tokio_tungstenite::tungstenite::handshake::server::Response
//! |-- tokio_tungstenite::tungstenite::protocol::CloseFrame
//! `-- tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/llm/src/route/transport/websocket.ts behavior.
//! Contract tests: provider-adapters/tests/transport_websocket.rs

use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use opencode_llm_api::{LlmError, LlmErrorReason};
use opencode_provider_adapters::protocol::Frame;
use opencode_provider_adapters::route::{HttpRequestParts, Transport};
use opencode_provider_adapters::transport::WebSocketJsonTransport;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[derive(Default, Debug)]
struct Seen {
    headers: Vec<(String, String)>,
    path: String,
    received: Vec<String>,
}

/// Accept one connection, record the handshake and the first message, then send `replies`
/// and close with `close` (None = drop the TCP connection without a close frame).
async fn server(replies: Vec<Message>, close: Option<u16>) -> (String, Arc<Mutex<Seen>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let record = seen.clone();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let hdr = record.clone();
        // The tungstenite `Callback` signature fixes the large `ErrorResponse` type.
        #[allow(clippy::result_large_err)]
        let callback = move |req: &Request, resp: Response| {
            let mut s = hdr.lock().unwrap();
            s.path = req.uri().to_string();
            s.headers = req
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
                .collect();
            Ok(resp)
        };
        let mut ws = tokio_tungstenite::accept_hdr_async(tcp, callback)
            .await
            .unwrap();
        if let Some(Ok(Message::Text(text))) = ws.next().await {
            record
                .lock()
                .unwrap()
                .received
                .push(text.as_str().to_owned());
        }
        for reply in replies {
            ws.send(reply).await.unwrap();
        }
        match close {
            Some(code) => {
                let _ = ws
                    .close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: "".into(),
                    }))
                    .await;
                while let Some(Ok(_)) = ws.next().await {}
            }
            None => drop(ws),
        }
    });
    (format!("http://{addr}/v1/responses?x=1"), seen)
}

fn transport() -> WebSocketJsonTransport {
    WebSocketJsonTransport::new(Arc::new(|body: Value| {
        let Value::Object(mut map) = body else {
            return Err(LlmError::invalid_request(
                "OpenAI Responses WebSocket body must be a JSON object",
            ));
        };
        map.shift_remove("stream");
        map.insert("type".into(), json!("response.create"));
        Ok(Value::Object(map).to_string())
    }))
}

fn parts(url: String) -> HttpRequestParts {
    HttpRequestParts {
        method: "POST".into(),
        url,
        headers: [
            ("authorization".to_string(), "Bearer sk-test".to_string()),
            (
                "openai-beta".to_string(),
                "responses_websockets=2026-02-06".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ]
        .into_iter()
        .collect(),
        body: String::new(),
        json_body: json!({"model": "gpt", "stream": true, "input": []}),
    }
}

async fn run(url: String) -> Vec<Result<Frame, LlmError>> {
    transport()
        .frames(parts(url), "openai/ws".into())
        .collect()
        .await
}

#[tokio::test]
async fn sends_message_and_yields_text_and_binary_frames() {
    let (url, seen) = server(
        vec![
            Message::text(r#"{"type":"response.created"}"#),
            Message::binary(br#"{"type":"response.completed"}"#.to_vec()),
        ],
        Some(1000),
    )
    .await;
    let frames: Vec<Frame> = run(url).await.into_iter().map(Result::unwrap).collect();
    assert_eq!(
        frames,
        [
            Frame::Text(r#"{"type":"response.created"}"#.into()),
            Frame::Text(r#"{"type":"response.completed"}"#.into()),
        ]
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.path, "/v1/responses?x=1");
    assert_eq!(
        seen.received,
        [r#"{"model":"gpt","input":[],"type":"response.create"}"#]
    );
    let header = |name: &str| {
        seen.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(header("authorization"), Some("Bearer sk-test"));
    assert_eq!(
        header("openai-beta"),
        Some("responses_websockets=2026-02-06")
    );
    assert_eq!(header("content-type"), None);
}

#[tokio::test]
async fn abnormal_close_code_fails_stream() {
    let (url, _) = server(vec![Message::text("{}")], Some(1011)).await;
    let frames = run(url).await;
    assert_eq!(frames.len(), 2);
    let error = frames[1].as_ref().unwrap_err();
    assert_eq!(error.module, "WebSocketExecutor");
    let LlmErrorReason::Transport { message, kind, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(message, "WebSocket closed with code 1011");
    assert_eq!(kind.as_deref(), Some("close"));
}

#[tokio::test]
async fn connection_drop_without_close_is_message_error() {
    let (url, _) = server(vec![Message::text("{}")], None).await;
    let frames = run(url).await;
    assert!(frames[0].is_ok());
    let error = frames.last().unwrap().as_ref().unwrap_err();
    let LlmErrorReason::Transport { kind, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(kind.as_deref(), Some("message"));
}

#[tokio::test]
async fn open_failure_is_open_transport_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let frames = run(format!("http://127.0.0.1:{port}/v1")).await;
    assert_eq!(frames.len(), 1);
    let error = frames[0].as_ref().unwrap_err();
    assert_eq!(
        (error.module.as_str(), error.method.as_str()),
        ("WebSocketExecutor", "open")
    );
    let LlmErrorReason::Transport {
        message, kind, url, ..
    } = error.reason.as_ref()
    else {
        panic!("{error:?}");
    };
    assert!(
        message.starts_with("Failed to open WebSocket: "),
        "{message}"
    );
    assert_eq!(kind.as_deref(), Some("open"));
    assert_eq!(
        url.as_deref(),
        Some(format!("ws://127.0.0.1:{port}/v1").as_str())
    );
}

#[tokio::test]
async fn unsupported_scheme_fails_before_connecting() {
    let frames = run("ftp://example.test/v1".into()).await;
    let error = frames[0].as_ref().unwrap_err();
    assert_eq!(error.method, "prepare");
    let LlmErrorReason::Transport { message, kind, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(message, "Unsupported WebSocket URL protocol ftp:");
    assert_eq!(kind.as_deref(), Some("websocket"));
}

#[tokio::test]
async fn to_message_failure_is_surfaced() {
    let mut request = parts("http://127.0.0.1:1/v1".into());
    request.json_body = json!([1]);
    let frames: Vec<_> = transport()
        .frames(request, "openai/ws".into())
        .collect()
        .await;
    let error = frames[0].as_ref().unwrap_err();
    assert_eq!(error.reason.tag(), "InvalidRequest");
}
