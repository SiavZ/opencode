//! Purpose: Basic/query-token authorization for current and legacy routes, with the PTY ticket
//! bypass and public UI asset exceptions. Mirrors `packages/server/src/auth.ts`,
//! `packages/server/src/middleware/authorization.ts` and the legacy httpapi authorization.
//! Scope: per-request check, process-level config.
//! Uses:
//! rust/crates/
//! |-- protocol/src/errors.rs
//! `-- server/src/middleware/errors.rs
//! External:
//! |-- axum::extract::Request
//! |-- axum::extract::State
//! |-- axum::http::Method::GET
//! |-- axum::http::header::AUTHORIZATION
//! |-- axum::middleware::Next
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- base64::engine::general_purpose::STANDARD_NO_PAD
//! |-- char::is_whitespace
//! `-- form_urlencoded::parse
//! Used by:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- server/src/app.rs
//! |-- server/src/middleware/mod.rs
//! |-- server/tests/http_auth.rs
//! |-- server/tests/http_cors.rs
//! |-- server/tests/http_legacy.rs
//! `-- server/tests/replay_baseline.rs
//! Invariants: `auth_token` query wins over the Authorization header; malformed credentials are
//! treated as empty; an empty or absent password disables enforcement; `/api/` rejections carry
//! the tagged UnauthorizedError body, every other rejection is an empty 401; both carry
//! `www-authenticate: Basic realm="Secure Area"`. OPTIONS never reaches this layer (CORS answers).
//! Contract tests: server/tests/http_auth.rs

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use opencode_protocol::errors::ApiError;

use crate::middleware::errors::HttpError;

const AUTH_TOKEN_QUERY: &str = "auth_token";
const TICKET_QUERY: &str = "ticket";
/// `packages/opencode/src/server/shared/public-ui.ts`.
const PUBLIC_UI_PATHS: [&str; 3] = [
    "/site.webmanifest",
    "/web-app-manifest-192x192.png",
    "/web-app-manifest-512x512.png",
];

/// `ServerAuth.Info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub username: String,
    pub password: Option<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            username: "opencode".to_string(),
            password: None,
        }
    }
}

impl AuthConfig {
    /// `OPENCODE_SERVER_PASSWORD` / `OPENCODE_SERVER_USERNAME` (default `opencode`), read when the
    /// listener is created so a refreshed environment is observed.
    pub fn from_env() -> Self {
        AuthConfig {
            username: std::env::var("OPENCODE_SERVER_USERNAME")
                .unwrap_or_else(|_| "opencode".to_string()),
            password: std::env::var("OPENCODE_SERVER_PASSWORD").ok(),
        }
    }

    /// `ServerAuth.required`.
    pub fn required(&self) -> bool {
        self.password.as_deref().is_some_and(|p| !p.is_empty())
    }

    /// `ServerAuth.authorized`.
    pub fn authorized(&self, username: &str, password: &str) -> bool {
        self.password.as_deref() == Some(password) && self.username == username
    }

    /// Credential decoded from the request, `("", "")` when absent or malformed.
    pub fn credential(query: Option<&str>, authorization: Option<&str>) -> (String, String) {
        let token = query.and_then(|q| {
            form_urlencoded::parse(q.as_bytes())
                .find(|(k, v)| k == AUTH_TOKEN_QUERY && !v.is_empty())
                .map(|(_, v)| v.into_owned())
        });
        if let Some(token) = token {
            return decode(&token);
        }
        let header = authorization.and_then(|h| {
            let (scheme, rest) = h.split_once(char::is_whitespace)?;
            scheme
                .eq_ignore_ascii_case("basic")
                .then(|| rest.trim_start().to_string())
        });
        match header {
            Some(value) if !value.is_empty() => decode(&value),
            _ => (String::new(), String::new()),
        }
    }
}

fn decode(input: &str) -> (String, String) {
    let engine = base64::engine::general_purpose::STANDARD;
    let lenient = base64::engine::general_purpose::STANDARD_NO_PAD;
    let bytes = engine
        .decode(input)
        .or_else(|_| lenient.decode(input.trim_end_matches('=')));
    let Ok(text) = bytes.map(|b| String::from_utf8_lossy(&b).into_owned()) else {
        return (String::new(), String::new());
    };
    match text.split_once(':') {
        Some((user, pass)) => (user.to_string(), pass.to_string()),
        None => (String::new(), String::new()),
    }
}

/// True for `/api/pty/:id/connect` or `/pty/:id/connect` with a non-empty `ticket` query.
pub fn has_pty_ticket(path: &str, query: Option<&str>) -> bool {
    let rest = path
        .strip_prefix("/api/pty/")
        .or_else(|| path.strip_prefix("/pty/"));
    let connect = rest
        .and_then(|r| r.strip_suffix("/connect"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'));
    connect
        && query.is_some_and(|q| {
            form_urlencoded::parse(q.as_bytes()).any(|(k, v)| k == TICKET_QUERY && !v.is_empty())
        })
}

/// axum middleware enforcing `config`.
pub async fn layer(
    State(config): State<Arc<AuthConfig>>,
    request: Request,
    next: Next,
) -> Response {
    if !config.required() {
        return next.run(request).await;
    }
    let uri = request.uri();
    let path = uri.path();
    let query = uri.query();
    if request.method() == axum::http::Method::GET && PUBLIC_UI_PATHS.contains(&path) {
        return next.run(request).await;
    }
    if has_pty_ticket(path, query) {
        return next.run(request).await;
    }
    let authorization = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let (username, password) = AuthConfig::credential(query, authorization);
    if config.authorized(&username, &password) {
        return next.run(request).await;
    }
    if path.starts_with("/api/") {
        return HttpError::from(ApiError::unauthorized()).into_response();
    }
    HttpError::UnauthorizedEmpty.into_response()
}
