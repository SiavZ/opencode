//! Purpose: CORS policy of the full host: origin allowlist from `packages/server/src/cors.ts`,
//! Effect `HttpMiddleware.cors` header shapes, the `corsVaryFix` Vary merge and the request
//! origin check used by PTY ticket issuance.
//! Scope: per-request, process-level configured extra origins.
//! Uses:
//! `-- none
//! External:
//! |-- axum::extract::Request
//! |-- axum::extract::State
//! |-- axum::http::HeaderMap
//! |-- axum::http::HeaderValue
//! |-- axum::http::Method
//! |-- axum::http::StatusCode
//! |-- axum::http::header
//! |-- axum::middleware::Next
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- header::ACCESS_CONTROL_ALLOW_HEADERS
//! |-- header::ACCESS_CONTROL_ALLOW_METHODS
//! |-- header::ACCESS_CONTROL_ALLOW_ORIGIN
//! |-- header::ACCESS_CONTROL_MAX_AGE
//! |-- header::ACCESS_CONTROL_REQUEST_HEADERS
//! |-- header::HeaderName
//! |-- header::ORIGIN
//! |-- header::VARY
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/middleware/mod.rs
//! |-- server/tests/http_auth.rs
//! |-- server/tests/http_cors.rs
//! |-- server/tests/http_legacy.rs
//! |-- server/tests/replay_baseline.rs
//! `-- server/tests/ws_pty.rs
//! Invariants: OPTIONS always answers 204 with allow-methods `GET, HEAD, PUT, PATCH, POST, DELETE`,
//! echoed request headers (`vary: Access-Control-Request-Headers`), max-age 86400, and for
//! allowed origins `access-control-allow-origin: <origin>` with `Vary` merged to include
//! `Origin`. Disallowed origins get no allow-origin header. Non-OPTIONS responses get
//! allow-origin and `Vary: Origin` (overwriting) for allowed or absent origins.
//! Contract tests: server/tests/http_cors.rs

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Extra allowed origins (`--cors` / config `server.cors`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorsConfig {
    pub origins: Vec<String>,
}

/// `isAllowedCorsOrigin`. A missing origin is allowed.
pub fn is_allowed_cors_origin(input: Option<&str>, config: &CorsConfig) -> bool {
    let Some(origin) = input else { return true };
    if origin.is_empty()
        || origin.starts_with("http://localhost:")
        || origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("oc://renderer")
        || matches!(
            origin,
            "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
        )
        || is_opencode_origin(origin)
    {
        return true;
    }
    config.origins.iter().any(|o| o == origin)
}

/// `isAllowedRequestOrigin`: same-host origins are also accepted.
pub fn is_allowed_request_origin(
    input: Option<&str>,
    host: Option<&str>,
    config: &CorsConfig,
) -> bool {
    let Some(origin) = input else { return true };
    if let Some(host) = host
        && origin_host(origin).is_some_and(|h| h == host)
    {
        return true;
    }
    is_allowed_cors_origin(Some(origin), config)
}

/// `^https://([a-z0-9-]+\.)*opencode\.ai$`.
fn is_opencode_origin(origin: &str) -> bool {
    let Some(host) = origin.strip_prefix("https://") else {
        return false;
    };
    if host == "opencode.ai" {
        return true;
    }
    let Some(prefix) = host.strip_suffix(".opencode.ai") else {
        return false;
    };
    prefix.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// `new URL(origin).host` for http(s)-like origins, `None` when unparsable.
fn origin_host(origin: &str) -> Option<String> {
    let (_, rest) = origin.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    let host = host
        .rsplit_once('@')
        .map_or(host, |(_, h)| h)
        .to_ascii_lowercase();
    // URL drops default ports.
    let host = host
        .strip_suffix(":80")
        .filter(|_| origin.starts_with("http://"))
        .or_else(|| {
            host.strip_suffix(":443")
                .filter(|_| origin.starts_with("https://"))
        })
        .map(str::to_string)
        .unwrap_or(host);
    Some(host)
}

const ALLOW_METHODS: &str = "GET, HEAD, PUT, PATCH, POST, DELETE";
const MAX_AGE: &str = "86400";

/// axum middleware implementing the full-host CORS layer.
pub async fn layer(
    State(config): State<Arc<CorsConfig>>,
    request: Request,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let allowed = origin
        .as_deref()
        .filter(|o| is_allowed_cors_origin(Some(o), &config))
        .map(str::to_string);
    if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        let headers = response.headers_mut();
        if let Some(origin) = &allowed {
            set(headers, header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        }
        set(headers, header::ACCESS_CONTROL_ALLOW_METHODS, ALLOW_METHODS);
        let requested = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty());
        match requested {
            Some(value) => {
                set(headers, header::VARY, "Access-Control-Request-Headers");
                set(headers, header::ACCESS_CONTROL_ALLOW_HEADERS, value);
            }
            None => set(headers, header::ACCESS_CONTROL_ALLOW_HEADERS, ""),
        }
        // Effect emits `Vary: Origin` only alongside an allow-origin header.
        set(headers, header::ACCESS_CONTROL_MAX_AGE, MAX_AGE);
        if allowed.is_some() {
            merge_vary_origin(headers);
        }
        if origin.is_none() {
            missing_origin_vary(headers);
        }
        return response;
    }
    let mut response = next.run(request).await;
    // Effect applies these in a pre-response handler with `setHeaders`, which overwrites any
    // `Vary` set by the handler.
    if let Some(origin) = &allowed {
        let headers = response.headers_mut();
        set(headers, header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        set(headers, header::VARY, "Origin");
    }
    if origin.is_none() {
        set(response.headers_mut(), header::VARY, "Origin");
    }
    response
}

/// Effect's predicate allowlist accepts a missing origin and spreads
/// `{ "access-control-allow-origin": undefined, vary: "Origin" }`: the undefined header is dropped
/// and `vary: Origin` survives (overwriting any earlier Vary in the preflight record).
fn missing_origin_vary(headers: &mut HeaderMap) {
    if headers.get(header::VARY).is_none() {
        set(headers, header::VARY, "Origin");
    }
}

fn set(headers: &mut HeaderMap, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

/// `corsVaryFix`: ensure `Vary` contains `Origin` when a concrete allow-origin is set.
fn merge_vary_origin(headers: &mut HeaderMap) {
    let Some(vary) = headers
        .get(header::VARY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
    else {
        set(headers, header::VARY, "Origin");
        return;
    };
    let has = vary
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .any(|t| t == "origin" || t == "*");
    if !has {
        set(headers, header::VARY, &format!("{vary}, Origin"));
    }
}
