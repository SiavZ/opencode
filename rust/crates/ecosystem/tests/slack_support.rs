//! Purpose: local axum Slack Web API and websocket fixture server with joined shutdown.
//! Scope: fixture tests only, binds 127.0.0.1 and uses synthetic credentials.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/slack/api.rs
//! External:
//! |-- axum::{Router, extract::ws, Json}
//! |-- futures_util::SinkExt
//! |-- serde_json::{Value, json}
//! |-- tokio::{net::TcpListener, sync::mpsc, task, time}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- ecosystem/tests/slack_bot.rs
//! `-- ecosystem/tests/slack_socket.rs
//! Invariants: no upstream calls, fixture server and upgraded sockets are stopped on exit.
//! Contract tests: ecosystem/tests/slack_bot.rs, ecosystem/tests/slack_socket.rs
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get, routing::post};
use futures_util::SinkExt;
use opencode_ecosystem::slack::api::SlackApi;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct Logs {
    pub posts: Mutex<Vec<Value>>,
    pub calls: Mutex<Vec<(String, String)>>,
    pub acks: Mutex<Vec<Value>>,
    pub sockets: AtomicUsize,
    pub active: AtomicUsize,
    pub pings: AtomicUsize,
    pub silent: AtomicBool,
    pub http_status: AtomicU16,
    pub retry_count: AtomicUsize,
    pub api_error: Mutex<Option<String>>,
    senders: Mutex<Vec<mpsc::Sender<Message>>>,
}

struct Active(Arc<Logs>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Server {
    pub api: SlackApi,
    pub logs: Arc<Logs>,
    pub cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct FixtureState {
    logs: Arc<Logs>,
    cancel: CancellationToken,
    websocket_url: String,
}

impl Server {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let logs = Arc::new(Logs::default());
        let cancel = CancellationToken::new();
        let state = FixtureState {
            logs: logs.clone(),
            cancel: cancel.clone(),
            websocket_url: format!("ws://{addr}/ws"),
        };
        let router = Router::new()
            .route("/api/{method}", post(api))
            .route("/ws", get(upgrade))
            .with_state(state);
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            api: SlackApi::new(format!("http://{addr}/api"), "app-fixture", "bot-fixture"),
            logs,
            cancel,
            task,
        }
    }

    pub async fn send(&self, value: Value) {
        self.send_raw(value.to_string()).await;
    }

    pub async fn send_raw(&self, text: String) {
        wait_for(|| !self.logs.senders.lock().unwrap().is_empty()).await;
        let tx = self.logs.senders.lock().unwrap().last().unwrap().clone();
        tx.send(Message::Text(text.into())).await.unwrap();
    }

    pub async fn close_socket(&self) {
        let tx = self.logs.senders.lock().unwrap().last().unwrap().clone();
        tx.send(Message::Close(None)).await.unwrap();
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        wait_for(|| self.logs.active.load(Ordering::SeqCst) == 0).await;
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn api(
    State(state): State<FixtureState>,
    Path(method): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let auth = headers["authorization"].to_str().unwrap().to_string();
    assert_eq!(
        auth,
        if method == "apps.connections.open" {
            "Bearer app-fixture"
        } else {
            "Bearer bot-fixture"
        }
    );
    state
        .logs
        .calls
        .lock()
        .unwrap()
        .push((method.clone(), auth));
    let status = state.logs.http_status.load(Ordering::SeqCst);
    if status > 0 {
        if state.logs.retry_count.load(Ordering::SeqCst) > 0 {
            state.logs.retry_count.fetch_sub(1, Ordering::SeqCst);
            if state.logs.retry_count.load(Ordering::SeqCst) == 0 {
                state.logs.http_status.store(0, Ordering::SeqCst);
            }
        }
        return (
            StatusCode::from_u16(status).unwrap(),
            [("retry-after", "0")],
            Json(json!({"ok": false})),
        )
            .into_response();
    }
    if let Some(code) = state.logs.api_error.lock().unwrap().clone() {
        return Json(json!({ "ok": false, "error": code })).into_response();
    }
    match method.as_str() {
        "apps.connections.open" => {
            Json(json!({ "ok": true, "url": state.websocket_url })).into_response()
        }
        "auth.test" => {
            Json(json!({ "ok": true, "user_id": "U_BOT", "bot_id": "B_BOT" })).into_response()
        }
        "chat.postMessage" => {
            state.logs.posts.lock().unwrap().push(body);
            Json(json!({ "ok": true, "ts": "fixture" })).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn upgrade(State(state): State<FixtureState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket(socket, state))
}

async fn websocket(mut socket: WebSocket, state: FixtureState) {
    state.logs.active.fetch_add(1, Ordering::SeqCst);
    let _active = Active(state.logs.clone());
    let (tx, mut rx) = mpsc::channel(128);
    if socket
        .send(Message::Text(json!({"type":"hello"}).to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    state.logs.senders.lock().unwrap().push(tx);
    state.logs.sockets.fetch_add(1, Ordering::SeqCst);
    loop {
        tokio::select! {
            _ = state.cancel.cancelled() => break,
            outbound = rx.recv() => {
                let Some(message) = outbound else { break; };
                let close = matches!(message, Message::Close(_));
                if socket.send(message).await.is_err() || close { break; }
            }
            incoming = socket.recv(), if !state.logs.silent.load(Ordering::SeqCst) => {
                match incoming {
                    Some(Ok(Message::Text(text))) => state.logs.acks.lock().unwrap().push(serde_json::from_str(&text).unwrap()),
                    Some(Ok(Message::Ping(payload))) => {
                        state.logs.pings.fetch_add(1, Ordering::SeqCst);
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}

pub async fn wait_for(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture condition timed out");
}

pub fn envelope(id: &str, channel: &str, thread: &str, text: &str) -> Value {
    json!({ "envelope_id": id, "type": "events_api", "payload": {
        "event_id": id, "event": { "type":"message", "channel":channel, "thread_ts":thread, "ts":"999", "text":text, "user":"U_USER" }
    } })
}
