//! Purpose: exercise actual embedded Process HTTP composition and joined disposal.
//! Scope: isolated SQLite/config paths and in-process public request descriptors.
//! Uses:
//! rust/crates/
//! |-- client/src/errors.rs
//! |-- client/src/lib.rs
//! `-- sdk-embedded/src/lib.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! |-- tokio::test
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: no real user directories, listeners, credentials or installed applications.
//! Contract tests: self
#![cfg(feature = "embedded")]

use std::time::Duration;

use opencode_client::errors::{ApiError, ClientErrorReason};
use opencode_sdk_embedded::{OpenCode, ProcessOptions, RequestDescriptor};
use serde_json::json;

fn request(method: &'static str, path: &str) -> RequestDescriptor {
    RequestDescriptor {
        method,
        path: path.into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: None,
        success_status: 200,
        declared_statuses: &[400, 404],
        empty: false,
    }
}

#[tokio::test]
async fn real_health_and_session_persist_across_joined_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("project");
    std::fs::create_dir_all(&directory).unwrap();
    let options = ProcessOptions::isolated(&root.path().join("host"));
    let host = OpenCode::create_with_options(options.clone())
        .await
        .unwrap();
    assert_eq!(
        host.client
            .request(request("GET", "/api/health"))
            .await
            .unwrap(),
        json!({"healthy":true})
    );
    let health = opencode_client::current::health::get::execute_async(
        &host.client,
        &opencode_client::current::health::get::Input {},
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(health).unwrap(),
        json!({"healthy":true})
    );
    let mut create = request("POST", "/api/session");
    create.body = Some(json!({"location":{"directory":directory}}));
    let created = host.client.request(create).await.unwrap();
    let id = created["data"]["id"].as_str().unwrap();
    let read = request("GET", &format!("/api/session/{id}"));
    let before = host.client.request(read.clone()).await.unwrap();
    let client = host.client.clone();
    let mut stream = host
        .client
        .execute(&request("GET", &format!("/api/session/{id}/event")))
        .await
        .unwrap()
        .into_body();
    tokio::time::timeout(Duration::from_secs(5), host.shutdown())
        .await
        .expect("shutdown must not require the caller to poll or drop an existing stream");
    assert!(http_body_util::BodyExt::frame(&mut stream).await.is_none());
    tokio::time::timeout(Duration::from_secs(5), client.wait_closed())
        .await
        .unwrap();
    assert!(
        matches!(client.request(request("GET", "/api/health")).await, Err(ApiError::Client(error)) if error.reason == ClientErrorReason::Transport)
    );
    let reopened = OpenCode::create_with_options(options).await.unwrap();
    assert_eq!(reopened.client.request(read).await.unwrap(), before);
    reopened.shutdown().await;
}

#[tokio::test]
async fn drop_closes_cloned_clients_and_tools_and_joins_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options.environment.vars.insert(
        "OPENCODE_SERVER_PASSWORD".into(),
        "isolated-test-password".into(),
    );
    let host = OpenCode::create_with_options(options).await.unwrap();
    assert_eq!(
        host.client
            .request(request("GET", "/api/health"))
            .await
            .unwrap(),
        json!({"healthy":true})
    );
    let response = host
        .client
        .execute(&request("GET", "/api/event"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut body = response.into_body();
    let client = host.client.clone();
    let tools = host.tools.clone();
    drop(host);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            http_body_util::BodyExt::frame(&mut body)
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(tools.register(Vec::new()).is_err());
    assert!(
        matches!(client.request(request("GET", "/api/health")).await, Err(ApiError::Client(error)) if error.reason == ClientErrorReason::Transport)
    );
    tokio::time::timeout(Duration::from_secs(5), client.wait_closed())
        .await
        .unwrap();
}

#[tokio::test]
async fn declared_errors_remain_distinct_from_unexpected_status() {
    let root = tempfile::tempdir().unwrap();
    let host = OpenCode::create_with_options(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    let mut invalid = request("POST", "/api/session");
    invalid.body = Some(json!({"location":42}));
    assert!(matches!(
        host.client.request(invalid.clone()).await,
        Err(ApiError::Declared { status: 400, .. })
    ));
    invalid.declared_statuses = &[];
    assert!(
        matches!(host.client.request(invalid).await, Err(ApiError::Client(error)) if error.reason == ClientErrorReason::UnexpectedStatus && error.status == Some(400))
    );
    host.shutdown().await;
}
