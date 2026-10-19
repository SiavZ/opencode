//! Purpose: socket tests for Basic/query-token authorization on current and legacy paths.
//! Scope: live local listener, fixture services.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/cors.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- app::router
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- serde_json::Value
//! |-- str::to_string
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/server/src/middleware/authorization.ts and the legacy router
//! authorization (empty 401 with WWW-Authenticate).
//! Contract tests: this file.

mod support;

use std::sync::Arc;

use base64::Engine;
use opencode_server::app::{self, ServerConfig};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;

fn basic(user: &str, pass: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
    )
}

fn token(user: &str, pass: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
}

async fn full(password: Option<&str>) -> opencode_server::lifecycle::Listener {
    let auth = AuthConfig {
        username: "opencode".into(),
        password: password.map(str::to_string),
    };
    let services = support::services(Arc::new(support::FakeSessions::default()));
    support::start(app::router(
        services,
        ServerConfig::full_host(auth, CorsConfig::default()),
    ))
    .await
}

#[tokio::test]
async fn current_route_requires_credentials_with_tagged_body() {
    let server = full(Some("secret")).await;
    let url = format!("{}/api/health", server.url());
    let response = support::client().get(&url).send().await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["www-authenticate"],
        "Basic realm=\"Secure Area\""
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::json!({"_tag":"UnauthorizedError","message":"Authentication required"})
    );

    let ok = support::client()
        .get(&url)
        .header("authorization", basic("opencode", "secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.text().await.unwrap(), r#"{"healthy":true}"#);
    server.stop(false).await;
}

#[tokio::test]
async fn query_token_wins_over_header() {
    let server = full(Some("secret")).await;
    let base = format!("{}/api/health", server.url());
    let good_token = format!("{base}?auth_token={}", token("opencode", "secret"));
    let bad_header = support::client()
        .get(&good_token)
        .header("authorization", basic("x", "y"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_header.status(), 200);
    let bad_token = format!("{base}?auth_token={}", token("opencode", "nope"));
    let good_header = support::client()
        .get(&bad_token)
        .header("authorization", basic("opencode", "secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(good_header.status(), 401);
    server.stop(false).await;
}

#[tokio::test]
async fn wrong_username_and_malformed_credentials_are_rejected() {
    let server = full(Some("secret")).await;
    let url = format!("{}/api/health", server.url());
    for header in [
        basic("admin", "secret"),
        "Basic !!!".to_string(),
        "Bearer abc".to_string(),
    ] {
        let response = support::client()
            .get(&url)
            .header("authorization", header)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }
    server.stop(false).await;
}

#[tokio::test]
async fn empty_password_disables_auth() {
    let server = full(Some("")).await;
    let response = support::client()
        .get(format!("{}/api/health", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    server.stop(false).await;
}

#[tokio::test]
async fn legacy_paths_answer_empty_401_and_public_ui_assets_bypass() {
    let server = full(Some("secret")).await;
    let response = support::client()
        .get(format!("{}/doc", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["www-authenticate"],
        "Basic realm=\"Secure Area\""
    );
    assert_eq!(response.text().await.unwrap(), "");
    // Public manifest bypasses auth and reaches the UI fallback (no UI composed -> typed 503).
    let manifest = support::client()
        .get(format!("{}/site.webmanifest", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(manifest.status(), 503);
    server.stop(false).await;
}

#[tokio::test]
async fn pty_ticket_bypasses_credentials() {
    let server = full(Some("secret")).await;
    let with_ticket = support::client()
        .get(format!("{}/pty/pty_1/connect?ticket=abc", server.url()))
        .send()
        .await
        .unwrap();
    assert_ne!(with_ticket.status(), 401);
    let without = support::client()
        .get(format!("{}/pty/pty_1/connect", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(without.status(), 401);
    server.stop(false).await;
}
