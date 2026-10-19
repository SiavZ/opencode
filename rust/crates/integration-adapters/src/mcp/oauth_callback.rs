//! Purpose: loopback OAuth callback server, port of oauth-callback.ts: listens on
//! 127.0.0.1:<port><path> (default 19876 /mcp/oauth/callback), validates `state`, resolves the
//! matching pending wait, supports cancel-by-name, timeout, stop-when-idle and port reuse.
//! Scope: one server instance per owner (the baseline keeps module globals; here the owner holds
//! an `Arc<OAuthCallback>`). The axum serve task is owned and joined on stop.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/mcp/oauth_provider.rs
//! External:
//! |-- axum::Router::new
//! |-- axum::extract::Query
//! |-- axum::extract::State
//! |-- axum::http::StatusCode
//! |-- axum::http::Uri
//! |-- axum::response::Html
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- axum::serve
//! |-- oneshot::Receiver
//! |-- oneshot::Sender
//! |-- oneshot::channel
//! |-- str::to_string
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::net::TcpStream::connect
//! |-- tokio::spawn
//! |-- tokio::sync::oneshot
//! |-- tokio::task::JoinHandle
//! |-- tokio::time::timeout
//! |-- tokio_util::sync::CancellationToken
//! `-- tokio_util::sync::CancellationToken::new
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/mcp/service.rs
//! Invariants: missing state -> 400 CSRF page; unknown state -> 400; `error` param rejects the
//! pending wait with error_description || error; waits time out after 5 minutes; the server
//! stops once no waits remain; if the port is already bound, start is a no-op (another
//! instance owns it).
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use tokio::sync::oneshot;

use super::oauth_provider::parse_redirect_uri;

pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const HOST: &str = "127.0.0.1";

type Waiter = oneshot::Sender<Result<String, String>>;

#[derive(Default)]
struct Inner {
    pending: HashMap<String, Waiter>,
    name_to_state: HashMap<String, String>,
    server: Option<(
        tokio_util::sync::CancellationToken,
        tokio::task::JoinHandle<()>,
    )>,
    port: u16,
    path: String,
}

#[derive(Clone, Default)]
pub struct OAuthCallback {
    inner: Arc<Mutex<Inner>>,
    timeout: Option<Duration>,
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl OAuthCallback {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: shorter callback timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            inner: Arc::default(),
            timeout: Some(timeout),
        }
    }

    pub fn is_running(&self) -> bool {
        lock(&self.inner).server.is_some()
    }

    /// `ensureRunning(redirectUri)`.
    pub async fn ensure_running(&self, redirect_uri: Option<&str>) -> std::io::Result<()> {
        let (port, path) = parse_redirect_uri(redirect_uri);
        let restart = {
            let i = lock(&self.inner);
            i.server.is_some() && (i.port != port || i.path != path)
        };
        if restart {
            self.stop().await;
        }
        if self.is_running() || is_port_in_use(port).await {
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind((HOST, port)).await?;
        let stop = tokio_util::sync::CancellationToken::new();
        let router = axum::Router::new()
            .fallback(handle)
            .with_state(self.clone());
        let token = stop.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(token.cancelled_owned())
                .await;
        });
        let mut i = lock(&self.inner);
        i.port = port;
        i.path = path;
        i.server = Some((stop, task));
        Ok(())
    }

    /// `waitForCallback(state, name)`: resolves with the code, or an error message.
    pub async fn wait_for_callback(
        &self,
        state: &str,
        name: Option<&str>,
    ) -> Result<String, String> {
        self.register(state, name).wait().await
    }

    /// Register the wait synchronously (before the browser opens), then await it.
    pub fn register(&self, state: &str, name: Option<&str>) -> PendingCallback {
        let (tx, rx) = oneshot::channel();
        let mut i = lock(&self.inner);
        if let Some(n) = name {
            i.name_to_state.insert(n.to_string(), state.to_string());
        }
        i.pending.insert(state.to_string(), tx);
        PendingCallback {
            owner: self.clone(),
            state: state.to_string(),
            name: name.map(str::to_string),
            rx,
        }
    }
}

pub struct PendingCallback {
    owner: OAuthCallback,
    state: String,
    name: Option<String>,
    rx: oneshot::Receiver<Result<String, String>>,
}

impl PendingCallback {
    pub async fn wait(self) -> Result<String, String> {
        let (owner, state, name) = (self.owner, self.state, self.name);
        let name = name.as_deref();
        let state = state.as_str();
        match tokio::time::timeout(owner.timeout.unwrap_or(CALLBACK_TIMEOUT), self.rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("OAuth callback server stopped".into()),
            Err(_) => {
                {
                    let mut i = lock(&owner.inner);
                    i.pending.remove(state);
                    if let Some(n) = name {
                        i.name_to_state.remove(n);
                    }
                }
                owner.stop_if_idle();
                Err("OAuth callback timeout - authorization took too long".into())
            }
        }
    }
}

impl OAuthCallback {
    /// `cancelPending(name)`.
    pub fn cancel_pending(&self, name: &str) {
        let waiter = {
            let mut i = lock(&self.inner);
            let key = i
                .name_to_state
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.to_string());
            let w = i.pending.remove(&key);
            if w.is_some() {
                i.name_to_state.remove(name);
            }
            w
        };
        if let Some(w) = waiter {
            let _ = w.send(Err("Authorization cancelled".into()));
            self.stop_if_idle();
        }
    }

    fn stop_if_idle(&self) {
        let server = {
            let mut i = lock(&self.inner);
            if !i.pending.is_empty() {
                return;
            }
            i.server.take()
        };
        if let Some((stop, _task)) = server {
            stop.cancel();
        }
    }

    /// `stop()`: shut the server down and reject all waits.
    pub async fn stop(&self) {
        let (server, pending) = {
            let mut i = lock(&self.inner);
            i.name_to_state.clear();
            (i.server.take(), std::mem::take(&mut i.pending))
        };
        if let Some((stop, task)) = server {
            stop.cancel();
            let _ = task.await;
        }
        for (_, w) in pending {
            let _ = w.send(Err("OAuth callback server stopped".into()));
        }
    }

    fn resolve(&self, state: &str, outcome: Result<String, String>) -> bool {
        let waiter = {
            let mut i = lock(&self.inner);
            let w = i.pending.remove(state);
            if w.is_some() {
                i.name_to_state.retain(|_, s| s != state);
            }
            w
        };
        match waiter {
            Some(w) => {
                let _ = w.send(outcome);
                true
            }
            None => false,
        }
    }
}

async fn handle(
    State(cb): State<OAuthCallback>,
    uri: axum::http::Uri,
    Query(q): Query<BTreeMap<String, String>>,
) -> axum::response::Response {
    let path = lock(&cb.inner).path.clone();
    if uri.path() != path {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let Some(state) = q.get("state").filter(|s| !s.is_empty()) else {
        return page(
            StatusCode::BAD_REQUEST,
            Some("Missing required state parameter - potential CSRF attack"),
        );
    };
    if let Some(error) = q.get("error").filter(|s| !s.is_empty()) {
        let msg = q
            .get("error_description")
            .filter(|s| !s.is_empty())
            .unwrap_or(error)
            .clone();
        cb.resolve(state, Err(msg.clone()));
        cb.stop_if_idle();
        return page(StatusCode::OK, Some(&msg));
    }
    let Some(code) = q.get("code").filter(|s| !s.is_empty()) else {
        return page(
            StatusCode::BAD_REQUEST,
            Some("No authorization code provided"),
        );
    };
    if !cb.resolve(state, Ok(code.clone())) {
        return page(
            StatusCode::BAD_REQUEST,
            Some("Invalid or expired state parameter - potential CSRF attack"),
        );
    }
    cb.stop_if_idle();
    page(StatusCode::OK, None)
}

/// Minimal self-contained callback page. The branded markup of
/// packages/core/src/oauth/page.ts is a recorded blocker (byte parity not ported).
fn page(status: StatusCode, error: Option<&str>) -> axum::response::Response {
    let body = match error {
        None => "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization successful</title></head><body><h1>Authorization successful</h1><p>OpenCode is now connected to MCP.</p><p>You can close this window.</p><script>setTimeout(()=>window.close(),2000)</script></body></html>".to_string(),
        Some(detail) => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization failed</title></head><body><h1>Authorization failed</h1><p>OpenCode couldn't finish connecting to MCP.</p><pre>{}</pre><p>Close this window and try again from OpenCode.</p></body></html>",
            escape_html(detail)
        ),
    };
    (
        status,
        [("content-type", "text/html; charset=utf-8")],
        Html(body),
    )
        .into_response()
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `isPortInUse`: something accepts connections on 127.0.0.1:port.
pub async fn is_port_in_use(port: u16) -> bool {
    tokio::net::TcpStream::connect((HOST, port)).await.is_ok()
}
