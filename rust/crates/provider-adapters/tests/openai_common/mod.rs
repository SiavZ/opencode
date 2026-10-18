//! Purpose: shared fixtures for the OpenAI protocol tests: a replay `Transport` that cuts recorded
//! SSE bodies through `transport::sse::SseParser` in small chunks and records the request it was
//! given, a cassette loader, and request builders mirroring packages/llm/test/recorded-scenarios.ts.
//! Scope: test-only helpers (fixture-based, no network).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/sse.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/tests/openai_chat.rs
//! |-- provider-adapters/tests/openai_recorded.rs
//! `-- provider-adapters/tests/openai_responses.rs
//! Invariants: replayed bodies are fed in 7-byte chunks so frame boundaries never align with
//! network chunks. Responses are served in recording order.
//! Contract tests: provider-adapters/tests/openai_recorded.rs

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use opencode_llm_api::{
    LlmError, LlmEvent, LlmRequest, LlmResponse, Model, RouteClient, SystemPart, ToolDefinition,
};
use opencode_provider_adapters::protocol::Frame;
use opencode_provider_adapters::route::{FrameStream, HttpRequestParts, ProtocolRoute, Transport};
use opencode_provider_adapters::transport::sse::SseParser;
use serde_json::{Value, json};

/// Replays SSE bodies in order and records the prepared requests.
#[derive(Debug, Default)]
pub struct Replay {
    responses: Mutex<VecDeque<String>>,
    pub seen: Mutex<Vec<HttpRequestParts>>,
}

impl Replay {
    pub fn new(bodies: impl IntoIterator<Item = String>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(bodies.into_iter().collect()),
            seen: Mutex::default(),
        })
    }

    pub fn bodies(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.body.clone())
            .collect()
    }
}

impl Transport for Replay {
    fn id(&self) -> &str {
        "http-json"
    }

    fn frames(&self, request: HttpRequestParts, route_label: String) -> FrameStream {
        self.seen.lock().unwrap().push(request);
        let body = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("replay ran out of recorded responses");
        let mut parser = SseParser::default();
        let mut frames: Vec<Result<Frame, LlmError>> = Vec::new();
        for chunk in body.as_bytes().chunks(7) {
            match parser.feed(chunk) {
                Ok(texts) => frames.extend(texts.into_iter().map(|t| Ok(Frame::Text(t)))),
                Err(e) => {
                    frames.push(Err(LlmError::invalid_output(
                        route_label.clone(),
                        format!("Failed to read {route_label} stream"),
                        Some(e.to_string()),
                    )));
                    break;
                }
            }
            if parser.ended() {
                break;
            }
        }
        Box::pin(futures::stream::iter(frames))
    }
}

/// One recorded HTTP interaction: (request url, request body, response body).
#[derive(Clone, Debug)]
pub struct Interaction {
    pub url: String,
    pub request_body: String,
    pub response_body: String,
}

pub fn cassette(name: &str) -> Vec<Interaction> {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let value: Value = serde_json::from_str(&text).unwrap();
    value["interactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| Interaction {
            url: i["request"]["url"].as_str().unwrap().to_owned(),
            request_body: i["request"]["body"].as_str().unwrap().to_owned(),
            response_body: i["response"]["body"].as_str().unwrap().to_owned(),
        })
        .collect()
}

/// SSE body from JSON events (`sseEvents` in packages/llm/test/lib/sse.ts).
pub fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("data: {e}\n\n"))
        .collect::<String>()
}

pub fn model(route: ProtocolRoute, id: &str, provider: &str) -> Model {
    Model::new(id, provider, Arc::new(route))
}

pub fn request(model: Model) -> LlmRequest {
    LlmRequest::new(model)
}

pub fn system(text: &str) -> Vec<SystemPart> {
    vec![SystemPart::text(text)]
}

pub fn weather_tool() -> ToolDefinition {
    ToolDefinition {
        name: "get_weather".into(),
        description: "Get current weather for a city.".into(),
        input_schema: json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"],
            "additionalProperties": false,
        })
        .as_object()
        .unwrap()
        .clone(),
        output_schema: None,
        cache: None,
        metadata: None,
        native: None,
    }
}

pub fn generation(
    max_tokens: f64,
    temperature: Option<f64>,
) -> opencode_llm_api::GenerationOptions {
    opencode_llm_api::GenerationOptions {
        max_tokens: Some(max_tokens),
        temperature,
        ..Default::default()
    }
}

pub fn provider_options(openai: Value) -> Option<opencode_llm_api::ProviderOptions> {
    Some(
        [("openai".to_owned(), openai.as_object().unwrap().clone())]
            .into_iter()
            .collect(),
    )
}

pub async fn generate(request: LlmRequest) -> Result<LlmResponse, LlmError> {
    opencode_llm_api::generate(&RouteClient, request).await
}

pub fn prepare(request: LlmRequest) -> Result<Value, LlmError> {
    use opencode_llm_api::LlmClient;
    RouteClient.prepare(request).map(|p| p.body)
}

pub fn kinds(events: &[LlmEvent]) -> Vec<&'static str> {
    events.iter().map(LlmEvent::kind).collect()
}

pub fn to_json(events: &[LlmEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}
