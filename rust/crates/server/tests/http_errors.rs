//! Purpose: socket tests for error envelopes and route mounting: unported operations answer the
//! typed 503, missing services never fake success, schema rejections and payload media types.
//! Scope: live local listener.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- server/src/app.rs
//! |-- server/src/services/mod.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- app::embedded_router
//! |-- reqwest::Method::from_bytes
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: every registry operation is reachable at its method and path.
//! Contract tests: this file.

mod support;

use opencode_server::app::{self, axum_path};
use opencode_server::services::Services;
use serde_json::{Value, json};

#[test]
fn path_templates_translate() {
    assert_eq!(
        axum_path("/api/session/:sessionID/message/:messageID"),
        "/api/session/{sessionID}/message/{messageID}"
    );
    assert_eq!(axum_path("/file/*"), "/file/{*wildcard}");
}

#[tokio::test]
async fn every_current_operation_is_mounted_and_typed_without_services() {
    let server = support::start(app::embedded_router(Services::default())).await;
    for (_, endpoint) in opencode_protocol::current::api().endpoints() {
        let path = endpoint
            .path
            .split('/')
            .map(|s| if s.starts_with(':') { "ses_x" } else { s })
            .collect::<Vec<_>>()
            .join("/");
        let method = reqwest::Method::from_bytes(endpoint.method.as_str().as_bytes()).unwrap();
        let response = support::client()
            .request(method, format!("{}{}", server.url(), path))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        assert_ne!(status, 404, "{} not mounted", endpoint.name);
        if endpoint.name == "health.get" {
            assert_eq!(status, 200);
            continue;
        }
        assert!(
            matches!(status, 400 | 503),
            "{} answered {status}",
            endpoint.name
        );
        let body: Value = response.json().await.unwrap();
        assert!(body["_tag"].is_string(), "{}: {body}", endpoint.name);
    }
    server.stop(false).await;
}

#[tokio::test]
async fn not_yet_ported_envelope_shape() {
    let server = support::start(app::embedded_router(Services::default())).await;
    let response = support::client()
        .get(format!("{}/api/location", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body,
        json!({"_tag":"ServiceUnavailableError","message":"Operation location is not available in this host yet","service":"location"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn unknown_path_is_404_on_embedded_router() {
    let server = support::start(app::embedded_router(Services::default())).await;
    let response = support::client()
        .get(format!("{}/nope", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let doc = support::client()
        .get(format!("{}/openapi.json", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(doc.status(), 200);
    let body: Value = doc.json().await.unwrap();
    assert_eq!(body["openapi"], "3.1.0");
    server.stop(false).await;
}
