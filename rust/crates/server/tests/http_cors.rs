//! Purpose: socket tests for the full-host CORS policy (packages/server/src/cors.ts, legacy
//! server.ts policy and corsVaryFix).
//! Scope: live local listener.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! `-- server/tests/support/mod.rs
//! External:
//! `-- reqwest::Client
//! Used by:
//! `-- none, entrypoint
//! Invariants: preflight answers 204 before auth; disallowed origins get no allow-origin.
//! Contract tests: this file.

mod support;

use std::sync::Arc;

use opencode_server::app::{self, ServerConfig};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::{CorsConfig, is_allowed_cors_origin};

async fn full() -> opencode_server::lifecycle::Listener {
    let auth = AuthConfig {
        username: "opencode".into(),
        password: Some("secret".into()),
    };
    let cors = CorsConfig {
        origins: vec!["https://example.com".into()],
    };
    let services = support::services(Arc::new(support::FakeSessions::default()));
    support::start(app::router(services, ServerConfig::full_host(auth, cors))).await
}

#[test]
fn origin_allowlist() {
    let config = CorsConfig {
        origins: vec!["https://example.com".into()],
    };
    for origin in [
        "http://localhost:3000",
        "http://127.0.0.1:1",
        "tauri://localhost",
        "https://opencode.ai",
        "https://app.dev.opencode.ai",
        "oc://renderer",
        "https://example.com",
    ] {
        assert!(is_allowed_cors_origin(Some(origin), &config), "{origin}");
    }
    for origin in [
        "https://evil.com",
        "https://opencode.ai.evil.com",
        "http://localhost",
        "https://APP.opencode.ai",
    ] {
        assert!(!is_allowed_cors_origin(Some(origin), &config), "{origin}");
    }
}

#[tokio::test]
async fn preflight_skips_auth_and_echoes_headers() {
    let server = full().await;
    let response = support::client()
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/api/health", server.url()),
        )
        .header("origin", "http://localhost:3000")
        .header(
            "access-control-request-headers",
            "authorization, content-type",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let headers = response.headers();
    assert_eq!(
        headers["access-control-allow-origin"],
        "http://localhost:3000"
    );
    assert_eq!(
        headers["access-control-allow-methods"],
        "GET, HEAD, PUT, PATCH, POST, DELETE"
    );
    assert_eq!(
        headers["access-control-allow-headers"],
        "authorization, content-type"
    );
    assert_eq!(headers["access-control-max-age"], "86400");
    assert_eq!(headers["vary"], "Access-Control-Request-Headers, Origin");
    server.stop(false).await;
}

#[tokio::test]
async fn disallowed_origin_gets_no_allow_origin() {
    let server = full().await;
    let response = support::client()
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/api/health", server.url()),
        )
        .header("origin", "https://evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    server.stop(false).await;
}

#[tokio::test]
async fn simple_request_gets_origin_headers_even_on_401() {
    let server = full().await;
    let response = support::client()
        .get(format!("{}/api/health", server.url()))
        .header("origin", "https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://example.com"
    );
    assert_eq!(response.headers()["vary"], "Origin");
    server.stop(false).await;
}
