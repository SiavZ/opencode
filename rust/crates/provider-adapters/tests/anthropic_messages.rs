//! Purpose: contract tests for the Anthropic Messages protocol: request body lowering parity with
//! packages/llm/test/provider/anthropic-messages.test.ts, SSE decoding of the same fixtures, and
//! replay of the recorded cassettes (anthropic-messages, anthropic-messages-cache) through the
//! real http-json transport with exact request-body equality.
//! Scope: fixture-based (local wiremock server), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/tests/fixtures/anthropic-support/cassette.rs
//! External:
//! `-- wiremock::{Mock, MockServer, ResponseTemplate}
//! Used by:
//! `-- none, entrypoint
//! Invariants: cassette bodies must match byte-for-byte (key order and number rendering).
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

#[path = "fixtures/anthropic-support/cassette.rs"]
mod cassette;

use std::sync::Arc;

use futures::StreamExt;
use opencode_llm_api::{
    CacheHint, CacheHintType, CachePolicy, ContentPart, FinishReason, GenerationOptions, LlmClient,
    LlmErrorReason, LlmEvent, LlmRequest, LlmResponse, MediaData, Message, MessageRole, Model,
    ProviderFailureClassification, RouteClient, SystemPart, ToolCallPart, ToolChoice, ToolContent,
    ToolDefinition, ToolResultPart, ToolResultValue, Usage, generate,
};
use opencode_provider_adapters::auth::{CredentialHeader, Secret, StaticCredential};
use opencode_provider_adapters::protocols::anthropic;
use opencode_provider_adapters::route::ProtocolRoute;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn route_at(
    base: &str,
    transport: Arc<dyn opencode_provider_adapters::route::Transport>,
) -> Arc<ProtocolRoute> {
    let mut route = anthropic::route(transport);
    route.endpoint.base_url = Some(base.to_owned());
    route.auth = Arc::new(CredentialHeader::header(
        "x-api-key",
        Arc::new(StaticCredential {
            source: "ANTHROPIC_API_KEY".into(),
            value: Some(Secret::new("test")),
        }),
    ));
    Arc::new(route)
}

fn offline_model(id: &str) -> Model {
    Model::new(
        id,
        "anthropic",
        route_at("https://api.anthropic.test/v1/", cassette::sse_transport()),
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

fn eph(ttl: Option<f64>) -> CacheHint {
    CacheHint {
        kind: CacheHintType::Ephemeral,
        ttl_seconds: ttl,
    }
}

fn gen_opts(max: f64, temperature: Option<f64>) -> Option<GenerationOptions> {
    Some(GenerationOptions {
        max_tokens: Some(max),
        temperature,
        ..Default::default()
    })
}

fn call(id: &str, name: &str, input: Value) -> ContentPart {
    ContentPart::ToolCall(ToolCallPart {
        id: id.into(),
        name: name.into(),
        input,
        provider_executed: None,
        metadata: None,
        provider_metadata: None,
    })
}

fn result(id: &str, name: &str, value: ToolResultValue) -> ToolResultPart {
    ToolResultPart {
        id: id.into(),
        name: name.into(),
        result: value,
        provider_executed: None,
        cache: None,
        metadata: None,
        provider_metadata: None,
    }
}

fn none_cache(mut r: LlmRequest) -> LlmRequest {
    r.cache = Some(CachePolicy::Named("none".into()));
    r
}

fn prepare(r: LlmRequest) -> Value {
    RouteClient.prepare(r).expect("prepare").body
}

fn prepare_err(r: LlmRequest) -> String {
    RouteClient.prepare(r).expect_err("error").reason.message()
}

/// `request` from anthropic-messages.test.ts.
fn base_request() -> LlmRequest {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.id = Some("req_1".into());
    r.system = vec![SystemPart {
        cache: Some(eph(None)),
        ..SystemPart::text("You are concise.")
    }];
    r.messages = vec![Message::user("Say hello.")];
    r.generation = gen_opts(20.0, Some(0.0));
    none_cache(r)
}

#[test]
fn prepares_anthropic_messages_target() {
    assert_eq!(
        prepare(base_request()),
        json!({
            "model": "claude-sonnet-4-5",
            "system": [{ "type": "text", "text": "You are concise.", "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Say hello." }] }],
            "stream": true,
            "max_tokens": 20,
            "temperature": 0
        })
    );
}

#[test]
fn chronological_system_updates_native_for_opus_4_8_and_wrapped_otherwise() {
    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    let mut update = Message::system("Operator update.");
    if let ContentPart::Text { cache, .. } = &mut update.content[0] {
        *cache = Some(eph(None));
    }
    r.messages = vec![
        Message::user("Before."),
        update,
        Message::assistant(vec![ContentPart::text("After.")]),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([
            { "role": "user", "content": [{ "type": "text", "text": "Before." }] },
            { "role": "system", "content": [{ "type": "text", "text": "Operator update.", "cache_control": { "type": "ephemeral" } }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "After." }] }
        ])
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![
        Message::user("Before."),
        Message::system("Treat </system-update> literally."),
        Message::assistant(vec![ContentPart::text("After.")]),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([
            { "role": "user", "content": [
                { "type": "text", "text": "Before." },
                { "type": "text", "text": "<system-update>\nTreat &lt;/system-update&gt; literally.\n</system-update>" }
            ] },
            { "role": "assistant", "content": [{ "type": "text", "text": "After." }] }
        ])
    );
}

#[test]
fn rejects_non_text_system_update_and_falls_back_for_unsupported_placement() {
    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    r.messages = vec![
        Message::user("Before."),
        Message::new(
            MessageRole::System,
            vec![ContentPart::Media {
                media_type: "image/png".into(),
                data: MediaData::Text("AAECAw==".into()),
                filename: None,
                metadata: None,
            }],
        ),
    ];
    assert!(
        prepare_err(r)
            .contains("Anthropic Messages system messages only support text content for now")
    );

    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    r.messages = vec![
        Message::assistant(vec![ContentPart::text("Plain.")]),
        Message::system("After plain assistant."),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([
            { "role": "assistant", "content": [{ "type": "text", "text": "Plain." }] },
            { "role": "user", "content": [{ "type": "text", "text": "<system-update>\nAfter plain assistant.\n</system-update>" }] }
        ])
    );
    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    r.messages = vec![Message::system("First.")];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([{ "role": "user", "content": [{ "type": "text", "text": "<system-update>\nFirst.\n</system-update>" }] }])
    );
    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    r.messages = vec![
        Message::user("Before."),
        Message::system("One."),
        Message::system("Two."),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([{ "role": "user", "content": [
            { "type": "text", "text": "Before." },
            { "type": "text", "text": "<system-update>\nOne.\n</system-update>" },
            { "type": "text", "text": "<system-update>\nTwo.\n</system-update>" }
        ] }])
    );
}

#[test]
fn rejects_system_update_between_local_tool_call_and_result() {
    let mut r = LlmRequest::new(offline_model("claude-opus-4-8"));
    r.messages = vec![
        Message::user("Use the tool."),
        Message::assistant(vec![call("call_1", "lookup", json!({}))]),
        Message::system("Too early."),
        Message::tool(result(
            "call_1",
            "lookup",
            ToolResultValue::Text(json!("Done.")),
        )),
    ];
    assert!(
        prepare_err(none_cache(r))
            .contains("system updates cannot split a local tool call from its tool result")
    );
}

#[test]
fn prepares_tool_call_and_tool_result_messages() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.id = Some("req_tool_result".into());
    r.messages = vec![
        Message::user("What is the weather?"),
        Message::assistant(vec![call(
            "call_1",
            "lookup",
            json!({ "query": "weather" }),
        )]),
        Message::tool(result(
            "call_1",
            "lookup",
            ToolResultValue::Json(json!({ "forecast": "sunny" })),
        )),
    ];
    assert_eq!(
        prepare(none_cache(r)),
        json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "What is the weather?" }] },
                { "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1", "name": "lookup", "input": { "query": "weather" } }] },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_1", "content": "{\"forecast\":\"sunny\"}" }] }
            ],
            "stream": true,
            "max_tokens": 4096
        })
    );
}

fn content_result(id: &str, name: &str, items: Vec<ToolContent>) -> Message {
    Message::tool(result(id, name, ToolResultValue::Content(items)))
}

fn file(uri: &str, mime: &str) -> ToolContent {
    ToolContent::File {
        uri: uri.into(),
        mime: mime.into(),
        name: None,
    }
}

#[test]
fn lowers_image_tool_results_as_structured_blocks_and_rejects_non_images() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![
        Message::user("Show me the screenshot."),
        Message::assistant(vec![call(
            "call_1",
            "read",
            json!({ "filePath": "shot.png" }),
        )]),
        content_result(
            "call_1",
            "read",
            vec![
                ToolContent::Text {
                    text: "Image read successfully".into(),
                },
                file("data:image/png;base64,AAECAw==", "image/png"),
            ],
        ),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"][2]["content"][0]["content"],
        json!([
            { "type": "text", "text": "Image read successfully" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAECAw==" } }
        ])
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![
        Message::assistant(vec![call("call_1", "screenshot", json!({}))]),
        content_result(
            "call_1",
            "screenshot",
            vec![file("data:image/jpeg;base64,/9j/AA==", "image/jpeg")],
        ),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"][1]["content"][0]["content"],
        json!([{ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "/9j/AA==" } }])
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![
        Message::assistant(vec![call("call_1", "fetch", json!({}))]),
        content_result(
            "call_1",
            "fetch",
            vec![file("data:audio/mpeg;base64,AAECAw==", "audio/mpeg")],
        ),
    ];
    let message = prepare_err(none_cache(r));
    assert!(
        message.contains("Anthropic Messages") && message.contains("audio/mpeg"),
        "{message}"
    );
}

#[test]
fn prepares_native_continuation_and_reasoning_signatures() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.system = vec![SystemPart::text(
        "You are concise. Continue from the provided history.",
    )];
    r.tools = vec![tool(
        "get_weather",
        "Get current weather for a city.",
        json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"], "additionalProperties": false }),
    )];
    r.messages = vec![
        Message::user("What is shown here?"),
        Message::assistant(vec![
            ContentPart::Reasoning {
                text: "I inspected the previous turn.".into(),
                encrypted: None,
                metadata: None,
                provider_metadata: Some(
                    [(
                        "anthropic".to_owned(),
                        obj(json!({ "signature": "sig_continuation_1" })),
                    )]
                    .into_iter()
                    .collect(),
                ),
            },
            ContentPart::text("It shows a small test image."),
        ]),
    ];
    r.messages[0].content.push(ContentPart::Media {
        media_type: "image/png".into(),
        data: MediaData::Text("AAECAw==".into()),
        filename: None,
        metadata: None,
    });
    let body = prepare(none_cache(r));
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": [
                { "type": "text", "text": "What is shown here?" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAECAw==" } }
            ] },
            { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "I inspected the previous turn.", "signature": "sig_continuation_1" },
                { "type": "text", "text": "It shows a small test image." }
            ] }
        ])
    );
    assert_eq!(body["tools"][0]["name"], "get_weather");
}

#[test]
fn round_trips_provider_executed_assistant_content() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    let mut server_call = ToolCallPart {
        id: "srvtoolu_abc".into(),
        name: "web_search".into(),
        input: json!({ "query": "effect 4" }),
        provider_executed: Some(true),
        metadata: None,
        provider_metadata: None,
    };
    let mut server_result = result(
        "srvtoolu_abc",
        "web_search",
        ToolResultValue::Json(json!([{ "url": "https://example.com" }])),
    );
    server_result.provider_executed = Some(true);
    r.messages = vec![
        Message::user("Search for something."),
        Message::assistant(vec![
            ContentPart::ToolCall(server_call.clone()),
            ContentPart::ToolResult(server_result.clone()),
            ContentPart::text("Found it."),
        ]),
        Message::user("Thanks."),
    ];
    let body = prepare(none_cache(r));
    assert_eq!(
        body["messages"][1]["content"],
        json!([
            { "type": "server_tool_use", "id": "srvtoolu_abc", "name": "web_search", "input": { "query": "effect 4" } },
            { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_abc", "content": [{ "url": "https://example.com" }] },
            { "type": "text", "text": "Found it." }
        ])
    );

    server_result.name = "future_server_tool".into();
    server_call.id = "x".into();
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![Message::assistant(vec![ContentPart::ToolResult(
        server_result,
    )])];
    assert!(prepare_err(r).contains("future_server_tool"));
}

#[test]
fn cache_control_ttl_tools_tool_results_and_breakpoint_cap() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.system = vec![SystemPart {
        cache: Some(eph(Some(3600.0))),
        ..SystemPart::text("system")
    }];
    r.messages = vec![Message::user("hi")];
    assert_eq!(
        prepare(r)["system"],
        json!([{ "type": "text", "text": "system", "cache_control": { "type": "ephemeral", "ttl": "1h" } }])
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    let mut lookup = tool(
        "lookup",
        "lookup tool",
        json!({ "type": "object", "properties": {} }),
    );
    lookup.cache = Some(eph(None));
    r.tools = vec![lookup];
    let mut tool_result = result(
        "call_1",
        "lookup",
        ToolResultValue::Json(json!({ "temp": 72 })),
    );
    tool_result.cache = Some(eph(None));
    r.messages = vec![
        Message::user("What's the weather?"),
        Message::assistant(vec![call("call_1", "lookup", json!({}))]),
        Message::tool(tool_result),
    ];
    let body = prepare(r);
    assert_eq!(
        body["tools"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );
    assert_eq!(
        body["messages"][2]["content"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );
    // Auto policy marks the latest user message too (3 breakpoints total).
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.system = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|t| SystemPart {
            cache: Some(eph(None)),
            ..SystemPart::text(*t)
        })
        .collect();
    r.messages = vec![Message::user("hi")];
    let body = prepare(r);
    let system = body["system"].as_array().unwrap();
    assert_eq!(
        system
            .iter()
            .filter(|p| p.get("cache_control").is_some())
            .count(),
        4
    );
    assert!(system[4].get("cache_control").is_none() && system[5].get("cache_control").is_none());

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.tools = ["t1", "t2", "t3", "t4"]
        .iter()
        .map(|n| {
            let mut t = tool(n, n, json!({ "type": "object", "properties": {} }));
            t.cache = Some(eph(None));
            t
        })
        .collect();
    r.system = vec![SystemPart {
        cache: Some(eph(None)),
        ..SystemPart::text("system-tail")
    }];
    let mut tail = Message::user("message-tail");
    if let ContentPart::Text { cache, .. } = &mut tail.content[0] {
        *cache = Some(eph(None));
    }
    r.messages = vec![tail];
    let body = prepare(r);
    assert!(
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t.get("cache_control").is_some())
    );
    assert!(body["system"][0].get("cache_control").is_none());
    assert!(
        body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none()
    );
}

#[test]
fn thinking_tool_choice_and_generation_fields() {
    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.messages = vec![Message::user("hi")];
    r.tools = vec![tool("lookup", "Lookup", json!({ "type": "object" }))];
    r.tool_choice = Some(ToolChoice::parse("required"));
    r.generation = Some(GenerationOptions {
        max_tokens: Some(100.0),
        top_p: Some(0.5),
        top_k: Some(3.0),
        stop: Some(vec!["END".into()]),
        ..Default::default()
    });
    r.provider_options = Some(
        [(
            "anthropic".to_owned(),
            obj(json!({ "thinking": { "type": "enabled", "budgetTokens": 1024 } })),
        )]
        .into_iter()
        .collect(),
    );
    let body = prepare(none_cache(r));
    assert_eq!(body["tool_choice"], json!({ "type": "any" }));
    assert_eq!(body["top_p"], json!(0.5));
    assert_eq!(body["top_k"], json!(3));
    assert_eq!(body["stop_sequences"], json!(["END"]));
    assert_eq!(
        body["thinking"],
        json!({ "type": "enabled", "budget_tokens": 1024 })
    );
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "model",
            "messages",
            "tools",
            "tool_choice",
            "stream",
            "max_tokens",
            "top_p",
            "top_k",
            "stop_sequences",
            "thinking"
        ]
    );

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.tools = vec![tool("lookup", "Lookup", json!({ "type": "object" }))];
    r.tool_choice = Some(ToolChoice::parse("none"));
    let body = prepare(none_cache(r));
    assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());

    let mut r = LlmRequest::new(offline_model("claude-sonnet-4-5"));
    r.provider_options = Some(
        [(
            "anthropic".to_owned(),
            obj(json!({ "thinking": { "type": "enabled" } })),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        prepare_err(r),
        "Anthropic thinking provider option requires budgetTokens"
    );
}

/// Serve `events` as an SSE body and run one generate call.
async fn run_sse(events: &[Value], request: impl FnOnce(Model) -> LlmRequest) -> LlmResponse {
    let server = MockServer::start().await;
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/messages"))
        .and(wiremock::matchers::header("x-api-key", "test"))
        .and(wiremock::matchers::header(
            "anthropic-version",
            "2023-06-01",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let model = Model::new(
        "claude-sonnet-4-5",
        "anthropic",
        route_at(&format!("{}/v1", server.uri()), cassette::sse_transport()),
    );
    generate(&RouteClient, request(model))
        .await
        .expect("generate")
}

fn with_model(model: Model) -> LlmRequest {
    let mut r = base_request();
    r.model = model;
    r
}

fn with_tool(name: &'static str) -> impl FnOnce(Model) -> LlmRequest {
    move |model| {
        let mut r = with_model(model);
        r.tools = vec![tool(name, "Lookup data", json!({ "type": "object" }))];
        r
    }
}

fn anthropic_meta(v: Value) -> Option<opencode_llm_api::ProviderMetadata> {
    Some([("anthropic".to_owned(), obj(v))].into_iter().collect())
}

#[tokio::test]
async fn parses_text_reasoning_and_usage_stream() {
    let response = run_sse(
        &[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 5, "cache_read_input_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "!" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "thinking_delta", "thinking": "thinking" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "signature_delta", "signature": "sig_1" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn", "stop_sequence": "\n\nHuman:" }, "usage": { "output_tokens": 2 } }),
            json!({ "type": "message_stop" }),
        ],
        with_model,
    )
    .await;
    assert_eq!(response.text(), "Hello!");
    assert_eq!(response.reasoning(), "thinking");
    let usage = response.usage.clone().unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.non_cached_input_tokens,
            usage.cache_read_input_tokens,
            usage.total_tokens
        ),
        (Some(6.0), Some(2.0), Some(5.0), Some(1.0), Some(8.0))
    );
    assert!(response.events.contains(&LlmEvent::ReasoningEnd {
        id: "reasoning-1".into(),
        provider_metadata: anthropic_meta(json!({ "signature": "sig_1" })),
    }));
    assert_eq!(
        serde_json::to_value(&response.message.content).unwrap(),
        json!([
            { "type": "text", "text": "Hello!" },
            { "type": "reasoning", "text": "thinking", "providerMetadata": { "anthropic": { "signature": "sig_1" } } }
        ])
    );
    let Some(LlmEvent::Finish {
        reason,
        provider_metadata,
        ..
    }) = response.events.last()
    else {
        panic!("finish")
    };
    assert_eq!(*reason, FinishReason::Stop);
    assert_eq!(
        *provider_metadata,
        anthropic_meta(json!({ "stopSequence": "\n\nHuman:" }))
    );
}

#[tokio::test]
async fn assembles_streamed_tool_call_input() {
    let response = run_sse(
        &[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 5 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call_1", "name": "lookup" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\"" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": ":\"weather\"}" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        ],
        with_tool("lookup"),
    )
    .await;
    let usage = Usage {
        input_tokens: Some(5.0),
        output_tokens: Some(1.0),
        non_cached_input_tokens: Some(5.0),
        total_tokens: Some(6.0),
        provider_metadata: anthropic_meta(json!({ "input_tokens": 5, "output_tokens": 1 })),
        ..Default::default()
    };
    let call = LlmEvent::ToolCall {
        id: "call_1".into(),
        name: "lookup".into(),
        input: json!({ "query": "weather" }),
        provider_executed: None,
        provider_metadata: None,
    };
    assert_eq!(
        response.events,
        vec![
            LlmEvent::StepStart { index: 0 },
            LlmEvent::ToolInputStart {
                id: "call_1".into(),
                name: "lookup".into(),
                provider_metadata: None
            },
            LlmEvent::ToolInputDelta {
                id: "call_1".into(),
                name: "lookup".into(),
                text: "{\"query\"".into()
            },
            LlmEvent::ToolInputDelta {
                id: "call_1".into(),
                name: "lookup".into(),
                text: ":\"weather\"}".into()
            },
            LlmEvent::ToolInputEnd {
                id: "call_1".into(),
                name: "lookup".into(),
                provider_metadata: None
            },
            call,
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
}

#[tokio::test]
async fn mid_stream_provider_errors() {
    let cases = [
        (
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
            "overloaded_error: Overloaded",
            None,
        ),
        (
            json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "prompt is too long: 210000 tokens" } }),
            "invalid_request_error: prompt is too long: 210000 tokens",
            Some(ProviderFailureClassification::ContextOverflow),
        ),
        (
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "" } }),
            "overloaded_error",
            None,
        ),
        (
            json!({ "type": "error" }),
            "Anthropic Messages stream error",
            None,
        ),
    ];
    for (event, message, classification) in cases {
        let response = run_sse(&[event], with_model).await;
        assert_eq!(
            response.events,
            vec![LlmEvent::ProviderError {
                message: message.into(),
                classification,
                retryable: None,
                provider_metadata: None
            }]
        );
    }
}

#[tokio::test]
async fn decodes_server_tool_use_and_results_as_provider_executed() {
    let response = run_sse(
        &[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 5 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "server_tool_use", "id": "srvtoolu_abc", "name": "web_search" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"effect 4\"}" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_abc", "content": [{ "type": "web_search_result", "url": "https://example.com", "title": "Example" }] } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Found it." } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 8 } }),
        ],
        with_tool("web_search"),
    )
    .await;
    assert!(response.events.contains(&LlmEvent::ToolCall {
        id: "srvtoolu_abc".into(),
        name: "web_search".into(),
        input: json!({ "query": "effect 4" }),
        provider_executed: Some(true),
        provider_metadata: None,
    }));
    assert!(response.events.contains(&LlmEvent::ToolResult {
        id: "srvtoolu_abc".into(),
        name: "web_search".into(),
        result: ToolResultValue::Json(json!([{ "type": "web_search_result", "url": "https://example.com", "title": "Example" }])),
        output: None,
        provider_executed: Some(true),
        provider_metadata: anthropic_meta(json!({ "blockType": "web_search_tool_result" })),
    }));
    assert_eq!(response.text(), "Found it.");
    assert_eq!(response.finish_reason, FinishReason::Stop);

    let response = run_sse(
        &[
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_x", "content": { "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded" } } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 1 } }),
        ],
        with_tool("web_search"),
    )
    .await;
    assert!(response.events.iter().any(|e| matches!(
        e,
        LlmEvent::ToolResult {
            result: ToolResultValue::Error(_),
            provider_executed: Some(true),
            ..
        }
    )));
}

#[tokio::test]
async fn http_errors_fail_before_stream_parsing() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"type":"error","error":{"type":"invalid_request_error","message":"Bad request"}}"#),
        )
        .mount(&server)
        .await;
    let model = Model::new(
        "claude-sonnet-4-5",
        "anthropic",
        route_at(&format!("{}/v1", server.uri()), cassette::sse_transport()),
    );
    let error = generate(&RouteClient, with_model(model))
        .await
        .expect_err("http 400");
    assert!(matches!(
        *error.reason,
        LlmErrorReason::InvalidRequest { .. }
    ));
    assert!(error.reason.message().contains("HTTP 400"));
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_request() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_delay(std::time::Duration::from_secs(30))
                .set_body_string("data: {\"type\":\"message_stop\"}\n\n"),
        )
        .mount(&server)
        .await;
    let model = Model::new(
        "claude-sonnet-4-5",
        "anthropic",
        route_at(&format!("{}/v1", server.uri()), cassette::sse_transport()),
    );
    let mut stream = RouteClient.stream(with_model(model));
    let next = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await;
    assert!(next.is_err(), "no event before the delayed response");
    drop(stream);
}

// ---------------------------------------------------------------------------
// Recorded cassettes (packages/llm/test/fixtures/recordings), replayed in order.
// ---------------------------------------------------------------------------

async fn replay_model(replay: &cassette::Replay, id: &str) -> Model {
    Model::new(
        id,
        "anthropic",
        route_at(&replay.base_url("/v1"), cassette::sse_transport()),
    )
}

fn weather_tool() -> ToolDefinition {
    tool(
        "get_weather",
        "Get current weather for a city.",
        json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"], "additionalProperties": false }),
    )
}

/// `eventSummary` (recorded-scenarios.ts) projection for recorded assertions.
fn summary(events: &[LlmEvent]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for event in events {
        match event {
            LlmEvent::TextDelta { text, .. } | LlmEvent::ReasoningDelta { text, .. } => {
                let kind = if matches!(event, LlmEvent::TextDelta { .. }) {
                    "text"
                } else {
                    "reasoning"
                };
                if let Some(last) = out.last_mut().filter(|l| l["type"] == kind) {
                    let value = format!("{}{text}", last["value"].as_str().unwrap());
                    last["value"] = json!(value);
                } else {
                    out.push(json!({ "type": kind, "value": text }));
                }
            }
            LlmEvent::ToolCall {
                name,
                input,
                provider_executed,
                ..
            } => {
                let mut v = json!({ "type": "tool-call", "name": name, "input": input });
                if let Some(p) = provider_executed {
                    v["providerExecuted"] = json!(p);
                }
                out.push(v);
            }
            LlmEvent::Finish { reason, usage, .. } => {
                let mut v = json!({ "type": "finish", "reason": reason });
                if let Some(u) = usage {
                    let mut s = serde_json::Map::new();
                    for (k, val) in [
                        ("inputTokens", u.input_tokens),
                        ("outputTokens", u.output_tokens),
                        ("reasoningTokens", u.reasoning_tokens),
                        ("cacheReadInputTokens", u.cache_read_input_tokens),
                        ("cacheWriteInputTokens", u.cache_write_input_tokens),
                        ("totalTokens", u.total_tokens),
                    ] {
                        if let Some(val) = val {
                            s.insert(k.into(), json!(val as i64));
                        }
                    }
                    v["usage"] = Value::Object(s);
                }
                out.push(v);
            }
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn recorded_streams_text() {
    let replay = cassette::Replay::start("anthropic-messages/streams-text.json").await;
    let mut r = LlmRequest::new(replay_model(&replay, "claude-haiku-4-5-20251001").await);
    r.system = vec![SystemPart::text("You are concise.")];
    r.messages = vec![Message::user("Reply with exactly: Hello!")];
    r.generation = gen_opts(20.0, Some(0.0));
    let response = generate(&RouteClient, none_cache(r)).await.unwrap();
    replay.assert_requests();
    assert_eq!(
        summary(&response.events),
        vec![
            json!({ "type": "text", "value": "Hello!" }),
            json!({ "type": "finish", "reason": "stop", "usage": { "inputTokens": 18, "outputTokens": 5, "cacheReadInputTokens": 0, "cacheWriteInputTokens": 0, "totalTokens": 23 } }),
        ]
    );
    let kinds: Vec<&str> = response.events.iter().map(LlmEvent::kind).collect();
    assert_eq!(
        kinds,
        [
            "step-start",
            "text-start",
            "text-delta",
            "text-end",
            "step-finish",
            "finish"
        ]
    );
}

#[tokio::test]
async fn recorded_streams_tool_call() {
    let replay = cassette::Replay::start("anthropic-messages/streams-tool-call.json").await;
    let mut r = LlmRequest::new(replay_model(&replay, "claude-haiku-4-5-20251001").await);
    r.system = vec![SystemPart::text("Call tools exactly as requested.")];
    r.messages = vec![Message::user("Call get_weather with city exactly Paris.")];
    r.tools = vec![weather_tool()];
    r.tool_choice = Some(ToolChoice::named("get_weather"));
    r.generation = gen_opts(80.0, Some(0.0));
    let response = generate(&RouteClient, none_cache(r)).await.unwrap();
    replay.assert_requests();
    assert_eq!(
        summary(&response.events),
        vec![
            json!({ "type": "tool-call", "name": "get_weather", "input": { "city": "Paris" } }),
            json!({ "type": "finish", "reason": "tool-calls", "usage": { "inputTokens": 677, "outputTokens": 33, "cacheReadInputTokens": 0, "cacheWriteInputTokens": 0, "totalTokens": 710 } }),
        ]
    );
}

#[tokio::test]
async fn recorded_opus_tool_loop() {
    let replay =
        cassette::Replay::start("anthropic-messages/claude-opus-4-7-drives-a-tool-loop.json").await;
    let model = replay_model(&replay, "claude-opus-4-7").await;
    let mut r = LlmRequest::new(model);
    r.system = vec![SystemPart::text(
        "Use the get_weather tool, then answer in one short sentence.",
    )];
    r.messages = vec![Message::user("What is the weather in Paris?")];
    r.tools = vec![weather_tool()];
    r.generation = gen_opts(80.0, None);
    let r = none_cache(r);
    let first = generate(&RouteClient, r.clone()).await.unwrap();
    let calls: Vec<ToolCallPart> = first
        .events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ToolCall {
                id, name, input, ..
            } => Some(ToolCallPart {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                provider_executed: None,
                metadata: None,
                provider_metadata: None,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].input, json!({ "city": "Paris" }));
    assert_eq!(first.finish_reason, FinishReason::ToolCalls);
    let mut next = r;
    next.messages
        .push(Message::assistant(vec![ContentPart::ToolCall(
            calls[0].clone(),
        )]));
    next.messages.push(Message::tool(result(
        &calls[0].id,
        "get_weather",
        ToolResultValue::Json(json!({ "temperature": 22, "condition": "sunny" })),
    )));
    let second = generate(&RouteClient, next).await.unwrap();
    replay.assert_requests();
    assert_eq!(second.finish_reason, FinishReason::Stop);
    assert!(second.text().contains("Paris"));
}

#[tokio::test]
async fn recorded_opus_image_tool_result() {
    let replay =
        cassette::Replay::start("anthropic-messages/anthropic-opus-4-7-image-tool-result.json")
            .await;
    let mut r = LlmRequest::new(replay_model(&replay, "claude-opus-4-7").await);
    r.system = vec![SystemPart::text(
        "Read images carefully. Reply only with the visible text, lowercase, no punctuation.",
    )];
    r.messages = vec![
        Message::user("Use the read_screenshot tool, then reply with the words shown."),
        Message::assistant(vec![call(
            "call_screenshot_1",
            "read_screenshot",
            json!({}),
        )]),
        content_result(
            "call_screenshot_1",
            "read_screenshot",
            vec![
                ToolContent::Text {
                    text: "Image read successfully".into(),
                },
                file(
                    &format!("data:image/png;base64,{}", cassette::restroom_png()),
                    "image/png",
                ),
            ],
        ),
    ];
    r.tools = vec![tool(
        "read_screenshot",
        "Capture a screenshot of the current screen.",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
    )];
    r.generation = gen_opts(40.0, None);
    let response = generate(&RouteClient, none_cache(r)).await.unwrap();
    replay.assert_requests();
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.text(), "jiggling restroom prison");
}

#[tokio::test]
async fn recorded_rejects_malformed_assistant_tool_order() {
    let replay = cassette::Replay::start(
        "anthropic-messages/rejects-malformed-assistant-tool-order-without-patch.json",
    )
    .await;
    let mut r = LlmRequest::new(replay_model(&replay, "claude-haiku-4-5-20251001").await);
    r.messages = vec![
        Message::assistant(vec![
            call("call_1", "get_weather", json!({ "city": "Paris" })),
            ContentPart::text("I will check the weather."),
        ]),
        Message::tool(result(
            "call_1",
            "get_weather",
            ToolResultValue::Json(json!({ "temperature": "72F" })),
        )),
        Message::user("Use that result to answer briefly."),
    ];
    r.tools = vec![tool(
        "get_weather",
        "Get weather",
        json!({ "type": "object", "properties": {} }),
    )];
    // The cassette predates the auto cache default (no cache_control on the wire).
    let error = generate(&RouteClient, none_cache(r))
        .await
        .expect_err("400");
    replay.assert_requests();
    assert!(matches!(
        *error.reason,
        LlmErrorReason::InvalidRequest { .. }
    ));
    assert!(error.reason.message().contains("HTTP 400"));
}

#[tokio::test]
async fn recorded_cache_write_then_read() {
    let replay = cassette::Replay::start(
        "anthropic-messages-cache/writes-then-reads-cache-control-on-identical-second-call.json",
    )
    .await;
    let mut r = LlmRequest::new(replay_model(&replay, "claude-haiku-4-5-20251001").await);
    r.id = Some("recorded_anthropic_cache".into());
    r.system = vec![SystemPart {
        cache: Some(eph(None)),
        ..SystemPart::text(cassette::large_cacheable_system())
    }];
    r.messages = vec![Message::user("Say hi.")];
    r.generation = gen_opts(16.0, Some(0.0));
    let r = none_cache(r);
    let first = generate(&RouteClient, r.clone()).await.unwrap();
    let second = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    let u1 = first.usage.unwrap();
    assert_eq!(
        (
            u1.cache_write_input_tokens,
            u1.cache_read_input_tokens,
            u1.input_tokens
        ),
        (Some(5752.0), Some(0.0), Some(5761.0))
    );
    let u2 = second.usage.unwrap();
    assert!(u2.cache_read_input_tokens.unwrap() > 0.0);
    assert_eq!(
        (u2.non_cached_input_tokens, u2.input_tokens, u2.total_tokens),
        (Some(9.0), Some(5761.0), Some(5766.0))
    );
}
