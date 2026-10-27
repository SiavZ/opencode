//! Purpose: verify standalone stat HTTP and native trace command entrypoints.
//! Scope: loopback fixture worker transport tests.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- http_body_util::BodyExt
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: empty IDs do not access MySQL; non-POST uses baseline 405 response.
//! Contract tests: this file.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use opencode_hosted_adapters::console::db::ConsoleDb;
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn console_workers_stat_empty_and_method_validation() {
    let db = ConsoleDb::connect("mysql://root@127.0.0.1:1/p12_no_connect").unwrap();
    let app = super::stat_router(db.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .body(Body::from(r#"{"ids":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "{}"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "Method Not Allowed"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .body(Body::from("invalid"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        super::run("stat", json!({"ids":[]}), &db).await.unwrap(),
        json!({})
    );
    assert_eq!(
        super::run("log-processor", json!([]), &db).await.unwrap(),
        json!({"console":[],"events":[]})
    );
    assert!(super::run("log-processor", json!({}), &db).await.is_err());
}
