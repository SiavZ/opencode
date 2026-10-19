//! Purpose: actual loopback acceptance for the additive full-request UI service boundary.
//! Scope: real server listener with a labeled UI port fixture; not production upstream acceptance.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/services/ports.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- axum::{body, http, response}
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: request method/URI/headers/body and response status/headers/body survive the port.
//! Contract tests: this file; production upstream and CSP tests belong to runtime.

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use opencode_server::app::{ServerConfig, router};
use opencode_server::services::{PortError, Services, UiAssets, UiRequest, UiResponse};
use serde_json::Value;

struct Static;
#[async_trait]
impl UiAssets for Static {
    async fn get(&self, path: &str) -> Option<(String, Vec<u8>, Option<String>)> {
        (path == "/static.css").then(|| ("text/css".into(), b"body{}".to_vec(), None))
    }
}

struct Proxy;
#[async_trait]
impl UiAssets for Proxy {
    async fn get(&self, _: &str) -> Option<(String, Vec<u8>, Option<String>)> {
        panic!("full request must not fall back to a path-only adapter")
    }
    async fn request(&self, request: UiRequest) -> Result<UiResponse, PortError> {
        if request.uri().path() == "/failure" {
            return Err(PortError {
                message: "controlled upstream transport failure".into(),
            });
        }
        assert_eq!(request.method(), "PATCH");
        assert_eq!(
            request.uri().path_and_query().unwrap().as_str(),
            "/proxy?tag=retained"
        );
        assert_eq!(request.headers()["x-client"], "original");
        let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes, b"original body"[..]);
        let mut response = (StatusCode::ACCEPTED, Body::from("actual response")).into_response();
        response
            .headers_mut()
            .insert("x-upstream", "preserved".parse().unwrap());
        Ok(response)
    }
}

#[tokio::test]
async fn existing_static_asset_adapters_remain_compatible() {
    let services = Services {
        ui: Some(Arc::new(Static)),
        ..Services::default()
    };
    let server = support::start(router(
        services,
        ServerConfig {
            full_host: true,
            ..ServerConfig::default()
        },
    ))
    .await;
    let response = support::client()
        .get(format!("{}/static.css", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/css");
    assert_eq!(response.text().await.unwrap(), "body{}");
    let response = support::client()
        .get(format!("{}/missing", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    server.stop(false).await;
}

#[tokio::test]
async fn full_ui_requests_preserve_wire_values_and_real_errors_use_defect_mapper() {
    let services = Services {
        ui: Some(Arc::new(Proxy)),
        ..Services::default()
    };
    let server = support::start(router(
        services,
        ServerConfig {
            full_host: true,
            ..ServerConfig::default()
        },
    ))
    .await;
    let response = support::client()
        .patch(format!("{}/proxy?tag=retained", server.url()))
        .header("x-client", "original")
        .body("original body")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(response.headers()["x-upstream"], "preserved");
    assert_eq!(response.text().await.unwrap(), "actual response");
    let response = support::client()
        .get(format!("{}/failure", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"], "UnknownError");
    assert_eq!(
        body["data"]["message"],
        "Unexpected server error. Check server logs for details."
    );
    assert!(body["data"]["ref"].as_str().unwrap().starts_with("err_"));
    server.stop(false).await;
}
