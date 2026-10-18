//! Purpose: public share API routes (`/`, `/share_create`, `/share_delete`, `/share_delete_admin`,
//! `/share_sync`, `/share_poll`, `/share_data`) ported from packages/function/src/api.ts:114-204
//! over the in-process `SyncNamespace` host.
//! Scope: HTTP and WebSocket mapping only. GitHub/Feishu routes in the same Worker belong to phase 14.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/public.rs
//! `-- hosted-domain/src/sharing/public.rs
//! External:
//! `-- axum::extract::ws::WebSocketUpgrade
//! Used by:
//! rust/bins/
//! |-- hosted/src/sharing/mod.rs
//! `-- hosted/src/sharing/tests.rs
//! Invariants: thrown errors (bad secret, bad admin secret, invalid JSON, orphan part) are plain
//! 500 `Internal Server Error` (hono default), `/share_sync` answers `{}` even when publish rejects
//! the key, `/share_poll` requires `Upgrade: websocket` (426) and `id` (400), share URLs use
//! `https://<WEB_DOMAIN>/s/<short>`.
//! Contract tests: bins/hosted/src/sharing/tests.rs

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use opencode_hosted_adapters::sharing::public::SyncNamespace;
use opencode_hosted_domain::sharing::public::{share_data, short_name};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone)]
pub struct PublicState {
    pub namespace: SyncNamespace,
    pub web_domain: String,
    /// `Resource.ADMIN_SECRET.value`.
    pub admin_secret: String,
}

pub fn routes() -> Router<PublicState> {
    Router::new()
        .route("/", get(|| async { text(StatusCode::OK, "Hello, world!") }))
        .route("/share_create", post(create))
        .route("/share_delete", post(delete))
        .route("/share_delete_admin", post(delete_admin))
        .route("/share_sync", post(sync))
        .route("/share_poll", get(poll))
        .route("/share_data", get(data))
}

fn text(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
        body,
    )
        .into_response()
}

fn internal() -> Response {
    text(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
}

fn ok_json(body: Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// `c.req.json<T>()` then field access: missing fields are `undefined`, rendered by JS as such.
fn body(bytes: &Bytes) -> Option<Value> {
    serde_json::from_slice::<Value>(bytes).ok()
}

fn field(body: &Value, name: &str) -> String {
    opencode_hosted_domain::sharing::share::js_string(body.get(name))
}

async fn create(State(state): State<PublicState>, bytes: Bytes) -> Response {
    let Some(body) = body(&bytes) else {
        return internal();
    };
    let session_id = field(&body, "sessionID");
    let short = short_name(&session_id);
    let secret = state.namespace.share(&short, &session_id).await;
    ok_json(json!({ "secret": secret, "url": format!("https://{}/s/{short}", state.web_domain) }))
}

async fn delete(State(state): State<PublicState>, bytes: Bytes) -> Response {
    let Some(body) = body(&bytes) else {
        return internal();
    };
    let name = short_name(&field(&body, "sessionID"));
    if state
        .namespace
        .assert_secret(&name, &field(&body, "secret"))
        .await
        .is_err()
    {
        return internal();
    }
    match state.namespace.clear(&name).await {
        Ok(()) => ok_json(json!({})),
        Err(_) => internal(),
    }
}

async fn delete_admin(State(state): State<PublicState>, bytes: Bytes) -> Response {
    let Some(body) = body(&bytes) else {
        return internal();
    };
    if field(&body, "adminSecret") != state.admin_secret {
        return internal();
    }
    match state
        .namespace
        .clear(&field(&body, "sessionShortName"))
        .await
    {
        Ok(()) => ok_json(json!({})),
        Err(_) => internal(),
    }
}

async fn sync(State(state): State<PublicState>, bytes: Bytes) -> Response {
    let Some(body) = body(&bytes) else {
        return internal();
    };
    let name = short_name(&field(&body, "sessionID"));
    if state
        .namespace
        .assert_secret(&name, &field(&body, "secret"))
        .await
        .is_err()
    {
        return internal();
    }
    let content = body.get("content").cloned().unwrap_or(Value::Null);
    match state
        .namespace
        .publish(&name, &field(&body, "key"), content)
        .await
    {
        Ok(_) => ok_json(json!({})),
        Err(_) => internal(),
    }
}

async fn poll(
    State(state): State<PublicState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let is_ws = headers.get(header::UPGRADE).and_then(|v| v.to_str().ok()) == Some("websocket");
    if !is_ws {
        return text(
            StatusCode::UPGRADE_REQUIRED,
            "Error: Upgrade header is required",
        );
    }
    let Some(id) = query.get("id").filter(|id| !id.is_empty()).cloned() else {
        return text(StatusCode::BAD_REQUEST, "Error: Share ID is required");
    };
    let Ok(upgrade) = upgrade else {
        return text(
            StatusCode::BAD_REQUEST,
            "Error: invalid WebSocket handshake",
        );
    };
    let mut frames = state.namespace.subscribe(&id).await;
    upgrade.on_upgrade(move |mut socket| async move {
        loop {
            tokio::select! {
                frame = frames.recv() => match frame {
                    Some(frame) => if socket.send(Message::Text(frame.into())).await.is_err() { break },
                    None => break,
                },
                incoming = socket.recv() => match incoming {
                    // webSocketMessage is a no-op, close is echoed by the runtime.
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                },
            }
        }
    })
}

async fn data(
    State(state): State<PublicState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(id) = query.get("id").filter(|id| !id.is_empty()) else {
        return text(StatusCode::BAD_REQUEST, "Error: Share ID is required");
    };
    let entries = state.namespace.get_data(id).await;
    match share_data(&entries) {
        Ok(body) => ok_json(body),
        Err(_) => internal(),
    }
}
