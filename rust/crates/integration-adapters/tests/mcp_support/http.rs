//! Purpose: in-process HTTP fixture peers for MCP tests: a Streamable HTTP MCP server (JSON or
//! SSE responses, session ids), a legacy HTTP+SSE MCP server, and an OAuth authorization server
//! (RFC 9728 + RFC 8414 metadata, dynamic registration, PKCE-checked authorize/token) guarding
//! a protected streamable endpoint.
//! Scope: test support only. Label: fixture.
//! Uses:
//! `-- none
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::body::Body::from_stream
//! |-- axum::extract::Query
//! |-- axum::extract::State
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- axum::routing::get
//! |-- axum::routing::post
//! |-- axum::serve
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! |-- futures::Stream
//! |-- futures::stream::unfold
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- sha2::Digest
//! |-- sha2::Sha256::digest
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc
//! |-- tokio::task::JoinHandle
//! |-- url::Url::parse
//! `-- url::form_urlencoded::parse
//! Used by:
//! rust/crates/
//! |-- integration-adapters/tests/mcp_http.rs
//! |-- integration-adapters/tests/mcp_oauth.rs
//! `-- integration-adapters/tests/mcp_support/mod.rs
//! Invariants: binds 127.0.0.1:0; all state in memory.
//! Contract tests: integration-adapters/tests/mcp_http.rs

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use serde_json::{Value, json};
use sha2::Digest;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Peer {
    pub base: String,
    /// Headers seen on MCP POSTs.
    pub headers: Mutex<Vec<HeaderMap>>,
    /// `/mcp` answers requests as SSE when true.
    pub sse_responses: Mutex<bool>,
    sse_clients: Mutex<HashMap<String, mpsc::Sender<String>>>,
    /// OAuth state.
    pub registration: Mutex<bool>,
    /// Registration endpoint answers 400 with a non-OAuth body mentioning registration.
    pub registration_broken: Mutex<bool>,
    codes: Mutex<HashMap<String, (String, String)>>,
    pub tokens: Mutex<Vec<String>>,
    pub registered: Mutex<Vec<Value>>,
}

impl Peer {
    /// End every open legacy SSE stream (server-side disconnect).
    pub fn drop_sse(&self) {
        self.sse_clients.lock().unwrap().clear();
    }
}

pub fn rpc(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "http-fixture", "version": "1"},
        }),
        "tools/list" => json!({"tools": [{"name": "ping", "inputSchema": {"type": "object"}}]}),
        "tools/call" => json!({"content": [{"type": "text", "text": "pong"}]}),
        _ => {
            return Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
            );
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

async fn streamable(State(p): State<Arc<Peer>>, headers: HeaderMap, body: String) -> Response {
    p.headers.lock().unwrap().push(headers.clone());
    let msg: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let Some(reply) = rpc(&msg) else {
        return StatusCode::ACCEPTED.into_response();
    };
    let session = [("mcp-session-id", "sess-1")];
    if *p.sse_responses.lock().unwrap() {
        let body = format!("event: message\ndata: {reply}\n\n");
        return (session, [("content-type", "text/event-stream")], body).into_response();
    }
    (
        session,
        [("content-type", "application/json")],
        reply.to_string(),
    )
        .into_response()
}

async fn protected(State(p): State<Arc<Peer>>, headers: HeaderMap, body: String) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = auth
        .strip_prefix("Bearer ")
        .is_some_and(|t| p.tokens.lock().unwrap().iter().any(|k| k == t));
    if !ok {
        let meta = format!("{}/.well-known/oauth-protected-resource/secure", p.base);
        let www = format!("Bearer resource_metadata=\"{meta}\"");
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", www)],
            "unauthorized",
        )
            .into_response();
    }
    streamable(State(p), headers, body).await
}

async fn legacy_get(State(p): State<Arc<Peer>>) -> Response {
    let (tx, mut rx) = mpsc::channel::<String>(16);
    p.sse_clients.lock().unwrap().insert("1".into(), tx);
    let stream = async_stream(
        move |out: mpsc::Sender<Result<String, std::io::Error>>| async move {
            let _ = out
                .send(Ok("event: endpoint\ndata: /messages?sid=1\n\n".into()))
                .await;
            while let Some(m) = rx.recv().await {
                let _ = out.send(Ok(format!("event: message\ndata: {m}\n\n"))).await;
            }
        },
    );
    (
        [("content-type", "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

fn async_stream<F, Fut>(f: F) -> impl futures::Stream<Item = Result<String, std::io::Error>>
where
    F: FnOnce(mpsc::Sender<Result<String, std::io::Error>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(f(tx));
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
}

async fn legacy_post(State(p): State<Arc<Peer>>, headers: HeaderMap, body: String) -> Response {
    p.headers.lock().unwrap().push(headers);
    let msg: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if let Some(reply) = rpc(&msg) {
        let tx = p.sse_clients.lock().unwrap().get("1").cloned();
        if let Some(tx) = tx {
            let _ = tx.send(reply.to_string()).await;
        }
    }
    StatusCode::ACCEPTED.into_response()
}

async fn resource_meta(State(p): State<Arc<Peer>>) -> Response {
    axum::Json(json!({"resource": format!("{}/secure", p.base), "authorization_servers": [p.base]}))
        .into_response()
}

async fn as_meta(State(p): State<Arc<Peer>>) -> Response {
    let mut m = json!({
        "issuer": p.base,
        "authorization_endpoint": format!("{}/authorize", p.base),
        "token_endpoint": format!("{}/token", p.base),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
    });
    if *p.registration.lock().unwrap() {
        m["registration_endpoint"] = json!(format!("{}/register", p.base));
    }
    axum::Json(m).into_response()
}

async fn register(State(p): State<Arc<Peer>>, body: String) -> Response {
    if *p.registration_broken.lock().unwrap() {
        return (
            StatusCode::BAD_REQUEST,
            "dynamic client registration is disabled",
        )
            .into_response();
    }
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    p.registered.lock().unwrap().push(v);
    axum::Json(json!({"client_id": "dyn-client", "client_id_issued_at": 1700000000}))
        .into_response()
}

/// Simulates the user approving: redirects to redirect_uri with a code bound to the challenge.
async fn authorize(
    State(p): State<Arc<Peer>>,
    Query(q): Query<BTreeMap<String, String>>,
) -> Response {
    let code = format!("code-{}", p.codes.lock().unwrap().len());
    let challenge = q.get("code_challenge").cloned().unwrap_or_default();
    let client = q.get("client_id").cloned().unwrap_or_default();
    p.codes
        .lock()
        .unwrap()
        .insert(code.clone(), (challenge, client));
    let mut url = url::Url::parse(&q["redirect_uri"]).unwrap();
    url.query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", q.get("state").map(String::as_str).unwrap_or(""));
    (StatusCode::FOUND, [("location", url.to_string())]).into_response()
}

async fn token(State(p): State<Arc<Peer>>, body: String) -> Response {
    let form: BTreeMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let Some((challenge, client)) = p
        .codes
        .lock()
        .unwrap()
        .remove(form.get("code").map(String::as_str).unwrap_or(""))
    else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    if computed != challenge || form.get("client_id") != Some(&client) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant", "error_description": "pkce"})),
        )
            .into_response();
    }
    let t = format!("tok-{}", p.tokens.lock().unwrap().len());
    p.tokens.lock().unwrap().push(t.clone());
    axum::Json(json!({"access_token": t, "token_type": "Bearer", "expires_in": 3600, "refresh_token": "r1"})).into_response()
}

pub async fn spawn_peer(registration: bool) -> (Arc<Peer>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let peer = Arc::new(Peer {
        base,
        registration: Mutex::new(registration),
        ..Default::default()
    });
    let app = Router::new()
        .route(
            "/mcp",
            post(streamable).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route(
            "/sse",
            get(legacy_get).post(|| async { StatusCode::NOT_FOUND }),
        )
        .route("/messages", post(legacy_post))
        .route(
            "/secure",
            post(protected).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route(
            "/.well-known/oauth-protected-resource/secure",
            get(resource_meta),
        )
        .route("/.well-known/oauth-authorization-server", get(as_meta))
        .route("/register", post(register))
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .with_state(peer.clone());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (peer, task)
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
