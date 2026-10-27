//! Purpose: accept migrated routing and unbuffered inference request replay.
//! Scope: local fake HTTP destination, isolated MySQL migration metadata and cancellation probes.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/gateway.rs
//! `-- hosted-adapters/src/console/gateway/body.rs
//! External:
//! |-- axum::Router
//! |-- futures::StreamExt
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only traffic; no live provider credentials or requests.
//! Contract tests: this file.

use axum::{
    Router,
    body::Bytes,
    extract::Request,
    http::{HeaderMap, Method},
    routing::any,
};
use futures::{StreamExt, TryStreamExt};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::{ConsoleDb, TestDatabase},
    gateway::{
        MigrationGateway,
        body::{BodyStream, PreparedBody, include_usage},
    },
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[tokio::test]
async fn fixture_model_replay_stops_at_top_level_model_and_escapes_replacement() {
    let polled = Arc::new(AtomicBool::new(false));
    let late = polled.clone();
    let stream: BodyStream = Box::pin(
        futures::stream::iter([Ok(Bytes::from_static(
            br#"{"nested":{"model":"wrong"},"model":"old""#,
        ))])
        .chain(futures::stream::once(async move {
            late.store(true, Ordering::SeqCst);
            Ok(Bytes::from_static(br#", "stream":true,"messages":[]}"#))
        })),
    );
    let prepared = PreparedBody::read(stream).await.unwrap();
    assert_eq!(prepared.model, "old");
    assert!(!polled.load(Ordering::SeqCst));
    let bytes = prepared
        .stream("native\"model")
        .unwrap()
        .try_fold(Vec::new(), |mut bytes, part| async move {
            bytes.extend_from_slice(&part);
            Ok(bytes)
        })
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["nested"]["model"], "wrong");
    assert_eq!(body["model"], "native\"model");
    assert!(polled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn fixture_missing_model_replay_is_explicit_error() {
    let stream: BodyStream = Box::pin(futures::stream::iter([Ok(Bytes::from_static(
        br#"{"messages":[]}"#,
    ))]));
    let prepared = PreparedBody::read(stream).await.unwrap();
    assert_eq!(prepared.model, "");
    assert!(prepared.stream("native").is_err());
}

#[tokio::test]
async fn fixture_include_usage_keeps_large_body_unbuffered_and_last_stream_flag() {
    let data =
        json!({"model":"m","stream":true,"messages":[{"content":"x".repeat(20000)}]}).to_string();
    let chunks = data
        .as_bytes()
        .chunks(17)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let bytes = include_usage(Box::pin(futures::stream::iter(chunks)))
        .try_fold(Vec::new(), |mut bytes, part| async move {
            bytes.extend_from_slice(&part);
            Ok(bytes)
        })
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["stream_options"], json!({"include_usage":true}));
    assert_eq!(
        body["messages"][0]["content"].as_str().unwrap().len(),
        20000
    );
}

#[tokio::test]
async fn fixture_new_keys_forward_without_legacy_database_and_preserve_destination_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(any(|request: Request| async move {
                assert_eq!(request.uri(), "/base/go/openai/v1/chat/completions?alt=sse");
                assert_eq!(request.headers()["authorization"], "Bearer oc_sk_local");
                assert_eq!(request.headers()["cf-access-client-id"], "trusted-id");
                assert_eq!(request.headers()["x-zen-ip"], "1.2.3.4");
                assert_eq!(request.headers()["x-opencode-request-id"], "req-local");
                assert!(request.headers().get("cf-access-client-secret").is_none());
                assert!(request.headers().get("x-zen-model").is_none());
                let bytes = axum::body::to_bytes(request.into_body(), 10000)
                    .await
                    .unwrap();
                assert_eq!(bytes, "request-body");
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "12"), ("content-type", "text/event-stream")],
                    "data: quota\n\n",
                )
            })),
        )
        .await
        .unwrap()
    });
    let gateway = MigrationGateway::new(&format!("http://{address}/base/"), "trusted-id").unwrap();
    let db = ConsoleDb::connect("mysql://nobody:unused@127.0.0.1:1/no_db").unwrap();
    let headers = HeaderMap::from_iter([
        (
            "authorization".parse().unwrap(),
            "Bearer oc_sk_local".parse().unwrap(),
        ),
        (
            "cf-connecting-ip".parse().unwrap(),
            "1.2.3.4".parse().unwrap(),
        ),
        ("x-zen-model".parse().unwrap(), "spoofed".parse().unwrap()),
        (
            "cf-access-client-secret".parse().unwrap(),
            "spoofed".parse().unwrap(),
        ),
        (
            "x-opencode-request".parse().unwrap(),
            "req-local".parse().unwrap(),
        ),
    ]);
    let target = gateway
        .target(
            &db,
            &Method::POST,
            "/zen/go/v1/chat/completions",
            Some("alt=sse"),
            &headers,
            None,
        )
        .await
        .unwrap()
        .unwrap();
    let response = gateway
        .forward(Method::POST, target, reqwest::Body::from("request-body"))
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "12");
    assert_eq!(response.text().await.unwrap(), "data: quota\n\n");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn fixture_migrated_legacy_byok_uses_connection_and_native_model_mapping() {
    let fixture = TestDatabase::create("gateway_migration").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name,migrated_at) VALUES ('wrk_gateway_migrate','Migration',now())").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`,time_deleted) VALUES ('key_gateway_migrate','wrk_gateway_migrate','usr_gateway','Migration','sk_legacy',now())").await.unwrap();
    conn.query_drop("INSERT INTO provider (id,workspace_id,provider,credentials) VALUES ('pro_gateway','wrk_gateway_migrate','anthropic','local-key')").await.unwrap();
    drop(conn);
    let gateway = MigrationGateway::new("http://127.0.0.1:1/base", "id").unwrap();
    let headers =
        HeaderMap::from_iter([("x-api-key".parse().unwrap(), "sk_legacy".parse().unwrap())]);
    let target = gateway
        .target(
            &fixture.db,
            &Method::POST,
            "/zen/v1/messages",
            None,
            &headers,
            Some(("anthropic", "native-model")),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target.url.path(),
        "/base/custom/conn_gateway_migrate_anthropic/messages"
    );
    assert_eq!(target.model.as_deref(), Some("native-model"));
    let missing =
        HeaderMap::from_iter([("x-api-key".parse().unwrap(), "sk_unknown".parse().unwrap())]);
    assert!(
        gateway
            .target(
                &fixture.db,
                &Method::POST,
                "/zen/v1/messages",
                None,
                &missing,
                None
            )
            .await
            .unwrap()
            .is_none()
    );
    fixture.drop_database().await.unwrap();
}

#[test]
fn fixture_route_mapping_rejects_unsupported_google_actions() {
    use opencode_hosted_domain::console::inference::migrated_path;
    assert_eq!(
        migrated_path("POST", "/zen/v1/models/gemini:streamGenerateContent").as_deref(),
        Some("/google/v1beta/models/gemini:streamGenerateContent")
    );
    assert!(migrated_path("POST", "/zen/v1/models/gemini:delete").is_none());
    assert!(migrated_path("POST", "/zen/v1/models/x/y:generateContent").is_none());
}
