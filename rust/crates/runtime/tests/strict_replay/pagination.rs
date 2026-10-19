//! Purpose: independently exercise persisted legacy message pagination through the full Process host.
//! Scope: production HTTP boundary with real SQLite rows, no injected service ports.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/strict_replay.rs
//! External:
//! |-- axum::body::Body
//! |-- axum::http::HeaderMap
//! |-- axum::http::Request::builder
//! |-- http_body_util::BodyExt
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::json
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tower::ServiceExt
//! Used by:
//! rust/crates/
//! `-- runtime/tests/strict_replay.rs
//! Invariants: follow the actual public cursor, preserve request origin/query, never insert test SQL.
//! Contract tests: this file

use axum::body::Body;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::Host;

async fn request(
    host: &Host,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, axum::http::HeaderMap, Value) {
    let response = host
        .router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("host", "parity.invalid")
                .header("x-forwarded-proto", "https")
                .header(
                    "x-opencode-directory",
                    &host
                        .literals
                        .iter()
                        .find(|(token, _)| token == "{PROJ}")
                        .unwrap()
                        .1,
                )
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&body).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_legacy_history_pages_have_exact_cursor_and_link_headers() {
    let host = Host::start(false).await;
    let created = request(
        &host,
        "POST",
        "/session",
        Some(json!({"title":"paging acceptance"})),
    )
    .await;
    assert_eq!(created.0, 200, "{}", created.2);
    let session = created.2["id"].as_str().unwrap();
    let mut ids = Vec::new();
    for text in ["older persisted message", "newer persisted message"] {
        let admitted = request(
            &host,
            "POST",
            &format!("/session/{session}/message"),
            Some(json!({
                "noReply":true,"parts":[{"type":"text","text":text}]
            })),
        )
        .await;
        assert_eq!(admitted.0, 200, "{}", admitted.2);
        ids.push(admitted.2["info"]["id"].as_str().unwrap().to_owned());
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let first = request(
        &host,
        "GET",
        &format!("/session/{session}/message?limit=1&tag=keep"),
        None,
    )
    .await;
    let cursor = first
        .1
        .get("x-next-cursor")
        .expect("two real rows must produce a cursor")
        .to_str()
        .unwrap()
        .to_owned();
    let second = request(
        &host,
        "GET",
        &format!("/session/{session}/message?limit=1&tag=keep&before={cursor}"),
        None,
    )
    .await;
    host.process.shutdown().await;
    assert_eq!(first.0, 200);
    assert_eq!(first.2.as_array().unwrap().len(), 1);
    assert_eq!(first.2[0]["info"]["id"], ids[1]);
    assert_eq!(first.2[0]["parts"][0]["text"], "newer persisted message");
    assert_eq!(
        first.1["access-control-expose-headers"],
        "Link, X-Next-Cursor"
    );
    assert_eq!(
        first.1["link"].to_str().unwrap(),
        format!(
            "<https://parity.invalid/session/{session}/message?limit=1&tag=keep&before={cursor}>; rel=\"next\""
        )
    );
    assert_eq!(second.0, 200);
    assert_eq!(second.2.as_array().unwrap().len(), 1);
    assert_eq!(second.2[0]["info"]["id"], ids[0]);
    assert_eq!(second.2[0]["parts"][0]["text"], "older persisted message");
    assert!(!second.1.contains_key("link"));
    assert!(!second.1.contains_key("x-next-cursor"));
}
