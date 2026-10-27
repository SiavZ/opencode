//! Purpose: verify authenticated native tail ingress over real loopback HTTP and durable capture.
//! Scope: native log worker fixture acceptance, no external service calls.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workers/tail.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/workers.rs
//! External:
//! |-- serde_json::{Value,json}
//! `-- tokio::net::TcpListener
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: invalid bearer cannot parse or capture data; successful response proves capture completed.
//! Contract tests: this file.

use opencode_hosted_adapters::console::workers::Capture;
use serde_json::{Value, json};

#[tokio::test]
async fn console_workers_tail_real_http_auth_limits_enrichment_and_capture() {
    let file =
        std::env::temp_dir().join(format!("console-workers-tail-{}.jsonl", std::process::id()));
    let router = super::tail_router(
        "fixture-tail-token".into(),
        Capture::JsonLines(file.clone()),
        1024,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/tail", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let response = client.post(&url).body("not-json").send().await.unwrap();
    assert_eq!(response.status(), 401);
    assert!(!file.exists());
    let response = client
        .post(&url)
        .bearer_auth("wrong")
        .body(" ".repeat(2048))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = client
        .post(&url)
        .bearer_auth("fixture-tail-token")
        .body(" ".repeat(1025))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let response = client
        .post(&url)
        .bearer_auth("fixture-tail-token")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let traces = json!([{"event":{"request":{"method":"POST","url":"https://example.test/zen/v1/responses","headers":{"x-real-ip":"2001:db8:abcd:1234::55","content-length":"42"}},"response":{"status":200}},"eventTimestamp":1793059200123_i64,"wallTime":50,"logs":[{"message":["_metric:{\"model\":\"fixture\"}"]}]}]);
    let response = client
        .post(&url)
        .bearer_auth("fixture-tail-token")
        .json(&traces)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"accepted":1})
    );
    let captured: Value =
        serde_json::from_str(std::fs::read_to_string(&file).unwrap().trim()).unwrap();
    assert_eq!(
        captured["console"][0]["ip.prefix"],
        "2001:db8:abcd:1234::/64"
    );
    assert_eq!(captured["events"][0]["data"]["model"], "fixture");
    let emitter = super::producer::TailEmitter::with_token(&url, "fixture-tail-token").unwrap();
    assert!(!format!("{emitter:?}").contains("fixture-tail-token"));
    assert!(
        super::producer::TailEmitter::with_token("https://api.example.test/tail", "fixture")
            .is_err()
    );
    emitter.emit(json!({"request":{"method":"POST","url":"https://example.test/zen/v1/responses","headers":{"x-real-ip":"203.0.113.9","content-length":"10"}},"response":{"status":201},"request_id":"request-fixture","eventTimestamp":1793059200123_i64,"wallTime":77,"model":"native-generated","tokens.output":5})).await.unwrap();
    let captured = std::fs::read_to_string(&file).unwrap();
    let last: Value = serde_json::from_str(captured.lines().last().unwrap()).unwrap();
    assert_eq!(last["console"][0]["model"], "native-generated");
    assert_eq!(last["console"][0]["request"], "request-fixture");
    assert_eq!(last["console"][0]["duration"], 77);
    assert_eq!(last["console"][0]["ip.prefix"], "203.0.113.9/32");
    assert!(last["console"][0].get("request_id").is_none());
    let invalid = super::producer::TailEmitter::with_token(&url, "wrong").unwrap();
    assert!(invalid.emit(json!({"request":{"method":"POST","url":"https://example.test/zen/v1/responses","headers":{}}})).await.is_err());
    task.abort();
    let _ = task.await;
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn console_workers_tail_capture_failures_are_redacted_and_empty_token_fails_closed() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let sink =
        Capture::JsonLines(std::env::temp_dir().join("missing-console-workers-parent/capture"));
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/tail")
            .header("authorization", "Bearer fixture")
            .body(Body::from("[]"))
            .unwrap()
    };
    let response = super::tail_router("fixture".into(), sink.clone(), 1024)
        .oneshot(request())
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let response = super::tail_router("".into(), sink, 1024)
        .oneshot(request())
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}
