//! Purpose: real loopback OpenAI-compatible provider for embedded application-tool acceptance.
//! Scope: deterministic tool call then text over HTTP/SSE, with captured request bodies.
//! Uses:
//! `-- none
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::extract::State
//! |-- axum::http::header
//! |-- axum::routing::post
//! |-- axum::serve
//! |-- header::CONTENT_TYPE
//! |-- header::HeaderName
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::task::JoinHandle
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- sdk-embedded/tests/embedded_tools.rs
//! Invariants: loopback only; server is cancelled and joined without live provider credentials.
//! Contract tests: sdk-embedded/tests/embedded_tools.rs

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::header;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub struct Provider {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Provider {
    pub async fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let app = Router::new()
            .route("/v1/chat/completions", post(reply))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(token.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            url,
            requests,
            cancel,
            task,
        }
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        self.task.await.unwrap();
    }
}

async fn reply(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> ([(header::HeaderName, &'static str); 1], String) {
    let index = {
        let mut requests = requests.lock().unwrap();
        requests.push(body);
        requests.len()
    };
    let tool = index % 2 == 1;
    let delta = if tool {
        json!({"tool_calls":[{"index":0,"id":"call_embedded","type":"function","function":{"name":"embedded_echo","arguments":"{\"text\":\"native-call\"}"}}]})
    } else {
        json!({"role":"assistant","content":"embedded tool completed"})
    };
    let frame = json!({"id":"chatcmpl-local","created":0,"model":"local-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
    let finish = json!({"id":"chatcmpl-local","created":0,"model":"local-test","choices":[{"index":0,"delta":{},"finish_reason":if tool {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        format!("data: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n"),
    )
}
