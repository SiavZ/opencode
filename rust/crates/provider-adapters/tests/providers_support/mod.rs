//! Purpose: shared wiremock fixtures for provider registry/facade tests: a real `HttpExecutor`
//! transport set, recorded-cassette SSE bodies, request capture helpers and a one-turn runner.
//! Scope: test-only helpers (fixture-based, wiremock on localhost, no live provider calls).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! |-- matchers::method
//! |-- matchers::path
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- wiremock::Mock
//! |-- wiremock::MockServer
//! |-- wiremock::Request
//! |-- wiremock::ResponseTemplate
//! `-- wiremock::matchers
//! Used by:
//! rust/crates/
//! |-- provider-adapters/tests/providers_cloud.rs
//! |-- provider-adapters/tests/providers_facades.rs
//! |-- provider-adapters/tests/providers_gateways.rs
//! `-- provider-adapters/tests/providers_registry.rs
//! Invariants: response bodies come from recorded cassettes under tests/fixtures or the TS
//! test helpers' `sseEvents` shape; the executor never retries (max_retries 0).
//! Contract tests: provider-adapters/tests/providers_registry.rs

#![allow(dead_code)]

use opencode_llm_api::{LlmRequest, LlmResponse, Message, Model, RouteClient, generate};
use opencode_provider_adapters::providers::Transports;
use opencode_provider_adapters::providers::config::{Env, fixed_env};
use opencode_provider_adapters::transport::HttpExecutor;
use serde_json::Value;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers};

pub fn transports() -> Transports {
    Transports::new(HttpExecutor::with_client(reqwest::Client::new()).max_retries(0))
}

pub fn env(pairs: &[(&str, &str)]) -> Env {
    fixed_env(pairs)
}

/// Response body of the first interaction of a recorded cassette (`tests/fixtures/<name>.json`).
pub fn cassette_body(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let value: Value = serde_json::from_str(&text).unwrap();
    value["interactions"][0]["response"]["body"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `sseEvents(...)` from packages/llm/test/lib/sse.ts.
pub fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

/// Minimal Chat Completions stream: "Hello" then stop (cloudflare.test.ts `deltaChunk`).
pub fn chat_hello() -> String {
    sse(&[
        serde_json::json!({ "id": "chatcmpl_fixture", "choices": [{ "delta": { "role": "assistant", "content": "Hello" }, "finish_reason": null }], "usage": null }),
        serde_json::json!({ "id": "chatcmpl_fixture", "choices": [{ "delta": {}, "finish_reason": "stop" }], "usage": null }),
    ])
}

/// Mount a POST mock at `path` returning `body` as an event stream.
pub async fn mount(server: &MockServer, path: &str, body: String) {
    Mock::given(matchers::method("POST"))
        .and(matchers::path(path))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

/// Run one "Say hello." turn against `model`.
pub async fn say_hello(model: Model) -> LlmResponse {
    let mut request = LlmRequest::new(model);
    request.messages = vec![Message::user("Say hello.")];
    generate(&RouteClient, request)
        .await
        .unwrap_or_else(|e| panic!("generate failed: {e:?}"))
}

/// Run one turn and return the error message.
pub async fn say_hello_err(model: Model) -> String {
    let mut request = LlmRequest::new(model);
    request.messages = vec![Message::user("Say hello.")];
    match generate(&RouteClient, request).await {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.reason.message(),
    }
}

/// The single captured request.
pub async fn only_request(server: &MockServer) -> Request {
    let mut requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    requests.remove(0)
}

pub fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
}

pub fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

/// Path plus query of a captured request.
pub fn path_and_query(request: &Request) -> String {
    match request.url.query() {
        Some(q) => format!("{}?{q}", request.url.path()),
        None => request.url.path().to_owned(),
    }
}
