//! Purpose: cassette replay harness for the messages-family protocol tests: loads
//! packages/llm recorded cassettes (copied under tests/fixtures), serves them in order from a
//! local wiremock server and checks each received request (method, path+query, exact body,
//! allow-listed headers) against the recording.
//! Scope: test support, fixture-based, no live provider calls.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/http.rs
//! External:
//! |-- base64::engine::general_purpose::STANDARD
//! `-- wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate}
//! Used by:
//! rust/crates/
//! |-- provider-adapters/tests/anthropic_messages.rs
//! |-- provider-adapters/tests/bedrock_converse.rs
//! `-- provider-adapters/tests/gemini.rs
//! Invariants: interactions replay strictly in recording order (identical cache requests get
//! the first then the second recorded response); unmatched extra requests receive HTTP 599.
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use opencode_provider_adapters::transport::{Framing, HttpExecutor, HttpJsonTransport};
use serde_json::Value;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Clone, Debug)]
pub struct Interaction {
    pub method: String,
    pub url: url::Url,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub status: u16,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Vec<u8>,
}

pub fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Load a cassette (`version: 1`, `interactions[]`).
pub fn load(rel: &str) -> Vec<Interaction> {
    let text = std::fs::read_to_string(fixture(rel)).expect("cassette");
    let doc: Value = serde_json::from_str(&text).expect("cassette json");
    doc["interactions"]
        .as_array()
        .expect("interactions")
        .iter()
        .map(|i| {
            let req = &i["request"];
            let res = &i["response"];
            let body = res["body"].as_str().unwrap_or_default();
            let response_body = if res["bodyEncoding"].as_str() == Some("base64") {
                STANDARD.decode(body).expect("base64 body")
            } else {
                body.as_bytes().to_vec()
            };
            Interaction {
                method: req["method"].as_str().unwrap_or("POST").to_owned(),
                url: url::Url::parse(req["url"].as_str().expect("url")).expect("url"),
                headers: pairs(&req["headers"]),
                body: req["body"].as_str().unwrap_or_default().to_owned(),
                status: res["status"].as_u64().unwrap_or(200) as u16,
                response_headers: pairs(&res["headers"]),
                response_body,
            }
        })
        .collect()
}

struct InOrder {
    interactions: Vec<Interaction>,
    next: AtomicUsize,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Respond for InOrder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.seen.lock().unwrap().push(request.clone());
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let Some(i) = self.interactions.get(index) else {
            return ResponseTemplate::new(599).set_body_string("cassette exhausted");
        };
        let mut template = ResponseTemplate::new(i.status);
        for (k, v) in &i.response_headers {
            template = template.insert_header(k.as_str(), v.as_str());
        }
        template.set_body_bytes(i.response_body.clone())
    }
}

pub struct Replay {
    pub server: MockServer,
    pub interactions: Vec<Interaction>,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Replay {
    pub async fn start(rel: &str) -> Self {
        let interactions = load(rel);
        let server = MockServer::start().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        Mock::given(wiremock::matchers::any())
            .respond_with(InOrder {
                interactions: interactions.clone(),
                next: AtomicUsize::new(0),
                seen: seen.clone(),
            })
            .mount(&server)
            .await;
        Self {
            server,
            interactions,
            seen,
        }
    }

    /// Base URL replacing the recorded origin, keeping the given path prefix (e.g. `/v1`).
    pub fn base_url(&self, prefix: &str) -> String {
        format!("{}{prefix}", self.server.uri())
    }

    /// Assert every recorded interaction was requested exactly as recorded.
    pub fn assert_requests(&self) {
        let seen = self.seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            self.interactions.len(),
            "request count differs from cassette"
        );
        for (index, (got, want)) in seen.iter().zip(&self.interactions).enumerate() {
            assert_eq!(got.method.as_str(), want.method, "method #{index}");
            assert_eq!(got.url.path(), want.url.path(), "path #{index}");
            assert_eq!(got.url.query(), want.url.query(), "query #{index}");
            let body = String::from_utf8(got.body.clone()).expect("utf8 body");
            pretty_assertions::assert_eq!(body, want.body, "body #{index}");
            for (k, v) in &want.headers {
                let actual = got.headers.get(k.as_str()).and_then(|h| h.to_str().ok());
                assert_eq!(actual, Some(v.as_str()), "header {k} #{index}");
            }
        }
    }

    pub fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

pub fn sse_transport() -> Arc<HttpJsonTransport> {
    Arc::new(HttpJsonTransport::sse(
        HttpExecutor::new().expect("executor").max_retries(0),
    ))
}

pub fn event_stream_transport(route: &str) -> Arc<HttpJsonTransport> {
    Arc::new(HttpJsonTransport::new(
        HttpExecutor::new().expect("executor").max_retries(0),
        Framing::aws_event_stream(route),
    ))
}

/// `restroom.png` used by the image scenarios, base64-encoded.
pub fn restroom_png() -> String {
    STANDARD.encode(std::fs::read(fixture("anthropic-support/restroom.png")).expect("png"))
}

/// `LARGE_CACHEABLE_SYSTEM` from packages/llm/test/recorded-scenarios.ts.
pub fn large_cacheable_system() -> String {
    "You are a concise, factual assistant. Answer precisely and avoid filler. Cite numbers when known. "
        .repeat(250)
}
