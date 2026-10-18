//! Purpose: contract tests for the Gemini protocol: body lowering parity with
//! packages/llm/test/provider/gemini.test.ts, SSE decoding of the same fixtures, and replay of
//! the recorded cassettes (gemini, gemini-cache) through the real http-json transport with exact
//! request-body equality.
//! Scope: fixture-based (local wiremock server), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/tests/fixtures/anthropic-support/cassette.rs
//! External:
//! `-- wiremock::{Mock, MockServer, ResponseTemplate}
//! Used by:
//! `-- none, entrypoint
//! Invariants: cassette bodies must match byte-for-byte (key order and number rendering).
//! Contract tests: provider-adapters/tests/gemini.rs

#[path = "fixtures/anthropic-support/cassette.rs"]
mod cassette;

use std::sync::Arc;

use opencode_llm_api::{
    ContentPart, FinishReason, GenerationOptions, LlmClient, LlmErrorReason, LlmEvent, LlmRequest,
    LlmResponse, MediaData, Message, Model, RouteClient, SystemPart, ToolCallPart, ToolChoice,
    ToolContent, ToolDefinition, ToolResultPart, ToolResultValue, Usage, generate,
};
use opencode_provider_adapters::auth::{CredentialHeader, Secret, StaticCredential};
use opencode_provider_adapters::protocols::gemini;
use opencode_provider_adapters::route::{ProtocolRoute, Transport};
use opencode_provider_adapters::shared::MAX_MEDIA_ENCODED_BYTES;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn route_at(base: &str, transport: Arc<dyn Transport>) -> Arc<ProtocolRoute> {
    let mut route = gemini::route(transport);
    route.endpoint.base_url = Some(base.to_owned());
    route.auth = Arc::new(CredentialHeader::header(
        "x-goog-api-key",
        Arc::new(StaticCredential {
            source: "GOOGLE_GENERATIVE_AI_API_KEY".into(),
            value: Some(Secret::new("test")),
        }),
    ));
    Arc::new(route)
}

fn offline_model() -> Model {
    Model::new(
        "gemini-2.5-flash",
        "google",
        route_at(
            "https://generativelanguage.test/v1beta/",
            cassette::sse_transport(),
        ),
    )
}

fn obj(v: Value) -> opencode_llm_api::JsonObject {
    v.as_object().cloned().unwrap()
}

fn tool(name: &str, description: &str, schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: obj(schema),
        output_schema: None,
        cache: None,
        metadata: None,
        native: None,
    }
}

fn call(id: &str, name: &str, input: Value) -> ToolCallPart {
    ToolCallPart {
        id: id.into(),
        name: name.into(),
        input,
        provider_executed: None,
        metadata: None,
        provider_metadata: None,
    }
}

fn tool_message(id: &str, name: &str, value: ToolResultValue) -> Message {
    Message::tool(ToolResultPart {
        id: id.into(),
        name: name.into(),
        result: value,
        provider_executed: None,
        cache: None,
        metadata: None,
        provider_metadata: None,
    })
}

fn media(media_type: &str, data: &str) -> ContentPart {
    ContentPart::Media {
        media_type: media_type.into(),
        data: MediaData::Text(data.into()),
        filename: None,
        metadata: None,
    }
}

fn gen_opts(max: f64, temperature: Option<f64>) -> Option<GenerationOptions> {
    Some(GenerationOptions {
        max_tokens: Some(max),
        temperature,
        ..Default::default()
    })
}

fn prepare(r: LlmRequest) -> Value {
    RouteClient.prepare(r).expect("prepare").body
}

fn prepare_err(r: LlmRequest) -> String {
    RouteClient.prepare(r).expect_err("error").reason.message()
}

fn base_request() -> LlmRequest {
    let mut r = LlmRequest::new(offline_model());
    r.id = Some("req_1".into());
    r.system = vec![SystemPart::text("You are concise.")];
    r.messages = vec![Message::user("Say hello.")];
    r.generation = gen_opts(20.0, Some(0.0));
    r
}

fn google(v: Value) -> Option<opencode_llm_api::ProviderMetadata> {
    Some([("google".to_owned(), obj(v))].into_iter().collect())
}

#[test]
fn prepares_gemini_target_and_route_path() {
    assert_eq!(
        prepare(base_request()),
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Say hello." }] }],
            "systemInstruction": { "parts": [{ "text": "You are concise." }] },
            "generationConfig": { "maxOutputTokens": 20, "temperature": 0 }
        })
    );
    assert_eq!(
        gemini::path("gemini-2.5-flash"),
        "/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );
}

#[test]
fn lowers_system_updates_multimodal_input_and_tool_history() {
    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![
        Message::user("Before."),
        Message::system("Update."),
        Message::assistant(vec![ContentPart::text("After.")]),
    ];
    assert_eq!(
        prepare(r)["contents"],
        json!([
            { "role": "user", "parts": [{ "text": "Before." }, { "text": "<system-update>\nUpdate.\n</system-update>" }] },
            { "role": "model", "parts": [{ "text": "After." }] }
        ])
    );

    let mut r = LlmRequest::new(offline_model());
    r.id = Some("req_tool_result".into());
    r.tools = vec![tool(
        "lookup",
        "Lookup data",
        json!({ "type": "object", "properties": { "query": { "type": "string" } } }),
    )];
    r.tool_choice = Some(ToolChoice::named("lookup"));
    r.messages = vec![
        Message::user("What is in this image?"),
        Message::assistant(vec![ContentPart::ToolCall(call(
            "call_1",
            "lookup",
            json!({ "query": "weather" }),
        ))]),
        tool_message(
            "call_1",
            "lookup",
            ToolResultValue::Json(json!({ "forecast": "sunny" })),
        ),
    ];
    r.messages[0].content.push(media("image/png", "AAECAw=="));
    assert_eq!(
        prepare(r),
        json!({
            "contents": [
                { "role": "user", "parts": [{ "text": "What is in this image?" }, { "inlineData": { "mimeType": "image/png", "data": "AAECAw==" } }] },
                { "role": "model", "parts": [{ "functionCall": { "name": "lookup", "args": { "query": "weather" } } }] },
                { "role": "user", "parts": [{ "functionResponse": { "name": "lookup", "response": { "name": "lookup", "content": "{\"forecast\":\"sunny\"}" } } }] }
            ],
            "tools": [{ "functionDeclarations": [{ "name": "lookup", "description": "Lookup data", "parameters": { "type": "object", "properties": { "query": { "type": "string" } } } }] }],
            "toolConfig": { "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["lookup"] } }
        })
    );
}

#[test]
fn image_tool_results_become_inline_data_and_data_urls_are_stripped() {
    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![
        Message::assistant(vec![ContentPart::ToolCall(call(
            "call_image",
            "read",
            json!({ "path": "pixel.png" }),
        ))]),
        tool_message(
            "call_image",
            "read",
            ToolResultValue::Content(vec![
                ToolContent::Text {
                    text: "Image read successfully".into(),
                },
                ToolContent::File {
                    uri: "data:image/png;base64,AAECAw==".into(),
                    mime: "image/png".into(),
                    name: Some("pixel.png".into()),
                },
            ]),
        ),
    ];
    let body = prepare(r);
    assert_eq!(
        body["contents"],
        json!([
            { "role": "model", "parts": [{ "functionCall": { "name": "read", "args": { "path": "pixel.png" } } }] },
            { "role": "user", "parts": [
                { "functionResponse": { "name": "read", "response": { "name": "read", "content": "Image read successfully" } } },
                { "inlineData": { "mimeType": "image/png", "data": "AAECAw==" } }
            ] }
        ])
    );
    assert!(
        !body["contents"]
            .to_string()
            .contains("\"content\":\"AAECAw==\"")
    );

    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![
        Message::new(
            opencode_llm_api::MessageRole::User,
            vec![media("image/png", "data:image/png;base64,AAEC")],
        ),
        tool_message(
            "call_image",
            "read",
            ToolResultValue::Content(vec![ToolContent::File {
                uri: "data:image/jpeg;base64,/9j/".into(),
                mime: "image/jpeg".into(),
                name: None,
            }]),
        ),
    ];
    assert_eq!(
        prepare(r)["contents"],
        json!([
            { "role": "user", "parts": [{ "inlineData": { "mimeType": "image/png", "data": "AAEC" } }] },
            { "role": "user", "parts": [
                { "functionResponse": { "name": "read", "response": { "name": "read", "content": "" } } },
                { "inlineData": { "mimeType": "image/jpeg", "data": "/9j/" } }
            ] }
        ])
    );
}

#[test]
fn rejects_invalid_media_and_unsupported_assistant_content() {
    for (media_type, data) in [
        ("image/png", "data:image/jpeg;base64,/9j/"),
        ("image/png", "%%%="),
        ("image/svg+xml", "PHN2Zz4="),
    ] {
        let mut r = LlmRequest::new(offline_model());
        r.messages = vec![Message::new(
            opencode_llm_api::MessageRole::User,
            vec![media(media_type, data)],
        )];
        let message = prepare_err(r);
        assert!(
            ["does not support", "does not match", "valid base64"]
                .iter()
                .any(|s| message.contains(s)),
            "{message}"
        );
    }
    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![Message::new(
        opencode_llm_api::MessageRole::User,
        vec![media("image/png", &"A".repeat(MAX_MEDIA_ENCODED_BYTES + 4))],
    )];
    assert!(prepare_err(r).contains("encoded limit"));

    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![Message::assistant(vec![media("image/png", "AAECAw==")])];
    assert!(prepare_err(r).contains(
        "Gemini assistant messages only support text, reasoning, and tool-call content for now"
    ));
}

#[test]
fn omits_tools_for_none_and_sanitizes_schema() {
    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![Message::user("Say hello.")];
    r.tools = vec![tool("lookup", "Lookup data", json!({ "type": "object" }))];
    r.tool_choice = Some(ToolChoice::parse("none"));
    assert_eq!(
        prepare(r),
        json!({ "contents": [{ "role": "user", "parts": [{ "text": "Say hello." }] }] })
    );

    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![Message::user("Use the tool.")];
    r.tools = vec![
        tool(
            "lookup",
            "Lookup data",
            json!({
                "type": "object",
                "required": ["status", "missing"],
                "properties": {
                    "status": { "type": "integer", "enum": [1, 2] },
                    "tags": { "type": "array" },
                    "name": { "type": "string", "properties": { "ignored": { "type": "string" } }, "required": ["ignored"] }
                }
            }),
        ),
        tool("noargs", "No args", json!({ "type": "object" })),
    ];
    let body = prepare(r);
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parameters"],
        json!({
            "type": "object",
            "required": ["status"],
            "properties": {
                "status": { "type": "string", "enum": ["1", "2"] },
                "tags": { "type": "array", "items": { "type": "string" } },
                "name": { "type": "string" }
            }
        })
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][1],
        json!({ "name": "noargs", "description": "No args" })
    );
    assert!(body.get("toolConfig").is_none());
}

#[test]
fn thinking_config_and_generation_fields() {
    let mut r = LlmRequest::new(offline_model());
    r.messages = vec![Message::user("hi")];
    r.generation = Some(GenerationOptions {
        top_p: Some(0.9),
        top_k: Some(40.0),
        stop: Some(vec!["X".into()]),
        ..Default::default()
    });
    r.provider_options = Some(
        [(
            "gemini".to_owned(),
            obj(json!({ "thinkingConfig": { "thinkingBudget": 0, "includeThoughts": "no" } })),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        prepare(r)["generationConfig"],
        json!({ "topP": 0.9, "topK": 40, "stopSequences": ["X"], "thinkingConfig": { "thinkingBudget": 0 } })
    );
}

async fn run_sse(
    body: String,
    request: impl FnOnce(Model) -> LlmRequest,
) -> Result<LlmResponse, opencode_llm_api::LlmError> {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent",
        ))
        .and(wiremock::matchers::query_param("alt", "sse"))
        .and(wiremock::matchers::header("x-goog-api-key", "test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let model = Model::new(
        "gemini-2.5-flash",
        "google",
        route_at(
            &format!("{}/v1beta", server.uri()),
            cassette::sse_transport(),
        ),
    );
    generate(&RouteClient, request(model)).await
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

fn with_model(model: Model) -> LlmRequest {
    let mut r = base_request();
    r.model = model;
    r
}

fn with_lookup(model: Model) -> LlmRequest {
    let mut r = with_model(model);
    r.tools = vec![tool("lookup", "Lookup data", json!({ "type": "object" }))];
    r
}

#[tokio::test]
async fn parses_text_reasoning_and_usage_stream() {
    let response = run_sse(
        sse(&[
            json!({ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "thinking", "thought": true }] } }] }),
            json!({ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "Hello" }] } }] }),
            json!({ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "!" }] }, "finishReason": "STOP" }] }),
            json!({ "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 2, "totalTokenCount": 7, "thoughtsTokenCount": 1, "cachedContentTokenCount": 1 } }),
        ]),
        with_model,
    )
    .await
    .unwrap();
    let usage = Usage {
        input_tokens: Some(5.0),
        output_tokens: Some(3.0),
        non_cached_input_tokens: Some(4.0),
        cache_read_input_tokens: Some(1.0),
        reasoning_tokens: Some(1.0),
        total_tokens: Some(7.0),
        provider_metadata: google(json!({
            "cachedContentTokenCount": 1,
            "thoughtsTokenCount": 1,
            "promptTokenCount": 5,
            "candidatesTokenCount": 2,
            "totalTokenCount": 7
        })),
        ..Default::default()
    };
    assert_eq!(response.text(), "Hello!");
    assert_eq!(response.reasoning(), "thinking");
    assert_eq!(
        response.events,
        vec![
            LlmEvent::StepStart { index: 0 },
            LlmEvent::ReasoningStart {
                id: "reasoning-0".into(),
                provider_metadata: None
            },
            LlmEvent::ReasoningDelta {
                id: "reasoning-0".into(),
                text: "thinking".into(),
                provider_metadata: None
            },
            LlmEvent::ReasoningEnd {
                id: "reasoning-0".into(),
                provider_metadata: None
            },
            LlmEvent::TextStart {
                id: "text-0".into(),
                provider_metadata: None
            },
            LlmEvent::TextDelta {
                id: "text-0".into(),
                text: "Hello".into(),
                provider_metadata: None
            },
            LlmEvent::TextDelta {
                id: "text-0".into(),
                text: "!".into(),
                provider_metadata: None
            },
            LlmEvent::TextEnd {
                id: "text-0".into(),
                provider_metadata: None
            },
            LlmEvent::StepFinish {
                index: 0,
                reason: FinishReason::Stop,
                usage: Some(usage.clone()),
                provider_metadata: None
            },
            LlmEvent::Finish {
                reason: FinishReason::Stop,
                usage: Some(usage),
                provider_metadata: None
            },
        ]
    );
}

#[tokio::test]
async fn preserves_thought_signatures_for_continuation() {
    let response = run_sse(
        sse(&[json!({ "candidates": [{ "content": { "role": "model", "parts": [
            { "text": "thinking", "thought": true },
            { "text": "", "thought": true, "thoughtSignature": "thought_sig" },
            { "functionCall": { "name": "lookup", "args": { "query": "weather" } }, "thoughtSignature": "tool_sig" }
        ] }, "finishReason": "STOP" }] })]),
        with_lookup,
    )
    .await
    .unwrap();
    let start = response
        .events
        .iter()
        .position(|e| matches!(e, LlmEvent::ReasoningStart { .. }))
        .unwrap();
    assert_eq!(
        response.events[start],
        LlmEvent::ReasoningStart {
            id: "reasoning-0".into(),
            provider_metadata: None
        }
    );
    let end = response
        .events
        .iter()
        .position(|e| matches!(e, LlmEvent::ReasoningEnd { .. }))
        .unwrap();
    let reasoning_meta = google(json!({ "thoughtSignature": "thought_sig" }));
    assert_eq!(
        response.events[end],
        LlmEvent::ReasoningEnd {
            id: "reasoning-0".into(),
            provider_metadata: reasoning_meta.clone()
        }
    );
    let tool_at = response
        .events
        .iter()
        .position(|e| matches!(e, LlmEvent::ToolCall { .. }))
        .unwrap();
    assert!(end < tool_at);
    let LlmEvent::ToolCall {
        provider_metadata: tool_meta,
        ..
    } = &response.events[tool_at]
    else {
        unreachable!()
    };
    assert_eq!(
        *tool_meta,
        google(json!({ "thoughtSignature": "tool_sig" }))
    );

    let mut r = LlmRequest::new(offline_model());
    let mut tool_call = call("tool_0", "lookup", json!({ "query": "weather" }));
    tool_call.provider_metadata = tool_meta.clone();
    r.messages = vec![Message::assistant(vec![
        ContentPart::Reasoning {
            text: "thinking".into(),
            encrypted: None,
            metadata: None,
            provider_metadata: reasoning_meta,
        },
        ContentPart::ToolCall(tool_call),
    ])];
    assert_eq!(
        prepare(r)["contents"],
        json!([{ "role": "model", "parts": [
            { "text": "thinking", "thought": true, "thoughtSignature": "thought_sig" },
            { "functionCall": { "name": "lookup", "args": { "query": "weather" } }, "thoughtSignature": "tool_sig" }
        ] }])
    );
}

#[tokio::test]
async fn emits_tool_calls_with_unique_ids_and_maps_finish() {
    let response = run_sse(
        sse(&[json!({
            "candidates": [{ "content": { "role": "model", "parts": [{ "functionCall": { "name": "lookup", "args": { "query": "weather" } } }] }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 1 }
        })]),
        with_lookup,
    )
    .await
    .unwrap();
    let usage = Usage {
        input_tokens: Some(5.0),
        output_tokens: Some(1.0),
        non_cached_input_tokens: Some(5.0),
        total_tokens: Some(6.0),
        provider_metadata: google(json!({ "promptTokenCount": 5, "candidatesTokenCount": 1 })),
        ..Default::default()
    };
    assert_eq!(
        response.events,
        vec![
            LlmEvent::StepStart { index: 0 },
            LlmEvent::ToolCall {
                id: "tool_0".into(),
                name: "lookup".into(),
                input: json!({ "query": "weather" }),
                provider_executed: None,
                provider_metadata: None
            },
            LlmEvent::StepFinish {
                index: 0,
                reason: FinishReason::ToolCalls,
                usage: Some(usage.clone()),
                provider_metadata: None
            },
            LlmEvent::Finish {
                reason: FinishReason::ToolCalls,
                usage: Some(usage),
                provider_metadata: None
            },
        ]
    );

    let response = run_sse(
        sse(&[
            json!({ "candidates": [{ "content": { "role": "model", "parts": [
            { "functionCall": { "name": "lookup", "args": { "query": "weather" } } },
            { "functionCall": { "name": "lookup", "args": { "query": "news" } } }
        ] }, "finishReason": "STOP" }] }),
        ]),
        with_lookup,
    )
    .await
    .unwrap();
    let ids: Vec<&str> = response
        .events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["tool_0", "tool_1"]);
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
}

#[tokio::test]
async fn maps_length_content_filter_and_partial_usage() {
    for (reason, expected) in [
        ("MAX_TOKENS", FinishReason::Length),
        ("SAFETY", FinishReason::ContentFilter),
        ("MALFORMED_FUNCTION_CALL", FinishReason::Error),
    ] {
        let response = run_sse(
            sse(&[json!({ "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": reason }] })]),
            with_model,
        )
        .await
        .unwrap();
        let kinds: Vec<&str> = response.events.iter().map(LlmEvent::kind).collect();
        assert_eq!(kinds, ["step-start", "step-finish", "finish"]);
        assert_eq!(response.finish_reason, expected);
    }
    let response = run_sse(
        sse(&[json!({ "usageMetadata": { "thoughtsTokenCount": 1 } })]),
        with_model,
    )
    .await
    .unwrap();
    let usage = response.usage.unwrap();
    assert_eq!(usage.reasoning_tokens, Some(1.0));
    assert_eq!(usage.total_tokens, None);
}

#[tokio::test]
async fn fails_invalid_stream_events() {
    let error = run_sse("data: {not json}\n\n".into(), with_model)
        .await
        .expect_err("invalid");
    assert!(matches!(
        *error.reason,
        LlmErrorReason::InvalidProviderOutput { .. }
    ));
    assert!(
        error
            .reason
            .message()
            .contains("Invalid google/gemini stream event")
    );
}

// ---------------------------------------------------------------------------
// Recorded cassettes.
// ---------------------------------------------------------------------------

fn replay_model(replay: &cassette::Replay) -> Model {
    Model::new(
        "gemini-2.5-flash",
        "google",
        route_at(&replay.base_url("/v1beta"), cassette::sse_transport()),
    )
}

fn finish_usage(response: &LlmResponse) -> (FinishReason, Usage) {
    (response.finish_reason, response.usage.clone().unwrap())
}

#[tokio::test]
async fn recorded_streams_text() {
    let replay = cassette::Replay::start("gemini/streams-text.json").await;
    let mut r = LlmRequest::new(replay_model(&replay));
    r.system = vec![SystemPart::text("You are concise.")];
    r.messages = vec![Message::user("Reply with exactly: Hello!")];
    r.generation = gen_opts(80.0, Some(0.0));
    let response = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    assert_eq!(response.text(), "Hello!");
    let (reason, usage) = finish_usage(&response);
    assert_eq!(reason, FinishReason::Stop);
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.reasoning_tokens,
            usage.total_tokens
        ),
        (Some(11.0), Some(18.0), Some(16.0), Some(29.0))
    );
}

#[tokio::test]
async fn recorded_streams_tool_call() {
    let replay = cassette::Replay::start("gemini/streams-tool-call.json").await;
    let mut r = LlmRequest::new(replay_model(&replay));
    r.system = vec![SystemPart::text("Call tools exactly as requested.")];
    r.messages = vec![Message::user("Call get_weather with city exactly Paris.")];
    r.tools = vec![tool(
        "get_weather",
        "Get current weather for a city.",
        json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"], "additionalProperties": false }),
    )];
    r.tool_choice = Some(ToolChoice::named("get_weather"));
    r.generation = gen_opts(80.0, Some(0.0));
    r.model.compatibility = None;
    let response = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    let calls: Vec<&LlmEvent> = response.tool_calls().collect();
    assert_eq!(calls.len(), 1);
    let LlmEvent::ToolCall {
        id,
        name,
        input,
        provider_metadata,
        ..
    } = calls[0]
    else {
        unreachable!()
    };
    assert_eq!(
        (id.as_str(), name.as_str(), input),
        ("tool_0", "get_weather", &json!({ "city": "Paris" }))
    );
    assert!(
        provider_metadata.as_ref().unwrap()["google"]["thoughtSignature"]
            .as_str()
            .unwrap()
            .starts_with("CiQBDDnW")
    );
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
}

#[tokio::test]
async fn recorded_reads_image_text() {
    let replay = cassette::Replay::start("gemini/gemini-2-5-flash-image.json").await;
    let mut r = LlmRequest::new(replay_model(&replay));
    r.system = vec![SystemPart::text(
        "Read images carefully. Reply only with the visible text.",
    )];
    r.messages = vec![Message::new(
        opencode_llm_api::MessageRole::User,
        vec![
            ContentPart::text(
                "The image contains exactly three lowercase English words. Read them left to right and reply with only those words.",
            ),
            media("image/png", &cassette::restroom_png()),
        ],
    )];
    r.generation = gen_opts(160.0, Some(0.0));
    let response = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    assert_eq!(response.text(), "jiggling restroom prison");
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

#[tokio::test]
async fn recorded_cache_reports_cached_content_tokens() {
    let replay = cassette::Replay::start(
        "gemini-cache/reports-cachedcontenttokencount-on-identical-second-call.json",
    )
    .await;
    let mut r = LlmRequest::new(replay_model(&replay));
    r.id = Some("recorded_gemini_cache".into());
    r.system = vec![SystemPart::text(cassette::large_cacheable_system())];
    r.messages = vec![Message::user("Say hi.")];
    r.generation = gen_opts(16.0, Some(0.0));
    let first = generate(&RouteClient, r.clone()).await.unwrap();
    let second = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    assert_eq!(first.usage.unwrap().cache_read_input_tokens, None);
    let u2 = second.usage.unwrap();
    assert_eq!(
        (u2.cache_read_input_tokens, u2.non_cached_input_tokens),
        (Some(1100.0), Some(100.0))
    );
}
