//! Purpose: actual HTTP acceptance for generated combined and current-only OpenAPI documents.
//! Scope: real loopback listener; registry/IR emitter executes without baseline files at runtime.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! `-- server/tests/support/mod.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: full-host docs merge legacy first; embedded docs advertise only current operations.
//! Contract tests: this file; protocol/tests/openapi.rs verifies individual emitted schema shapes.

mod support;

use opencode_server::app::{ServerConfig, router};
use opencode_server::services::Services;
use serde_json::Value;

#[tokio::test]
async fn full_host_document_contains_both_real_api_registries() {
    let server = support::start(router(
        Services::default(),
        ServerConfig {
            full_host: true,
            ..ServerConfig::default()
        },
    ))
    .await;
    let response = support::client()
        .get(format!("{}/doc", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["info"]["title"], "opencode");
    assert_eq!(
        body["paths"]["/api/health"]["get"]["operationId"],
        "v2.health.get"
    );
    assert_eq!(
        body["paths"]["/session"]["post"]["operationId"],
        "session.create"
    );
    assert!(body["paths"].as_object().unwrap().len() >= 160);
    assert!(body["components"]["schemas"]["UnknownError"].is_object());
    assert!(body["components"]["schemas"]["UnknownError1"].is_object());
    server.stop(false).await;
}

#[tokio::test]
async fn embedded_document_does_not_advertise_unmounted_legacy_routes() {
    let server = support::start(opencode_server::app::embedded_router(Services::default())).await;
    let response = support::client()
        .get(format!("{}/openapi.json", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["paths"]["/api/health"]["get"]["operationId"],
        "v2.health.get"
    );
    assert!(body["paths"].get("/session").is_none());
    server.stop(false).await;
}
