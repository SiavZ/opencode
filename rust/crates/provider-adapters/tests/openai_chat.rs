//! Purpose: OpenAI Chat protocol behavior tests ported from packages/llm/test/provider/openai-chat.test.ts
//! and openai-compatible-chat.test.ts: payload lowering (system updates, reasoning replay,
//! options, tool calls/results, tool-result vision aggregation, media validation) and stream
//! decoding (text/usage, reasoning_content, tool assembly, missing finish, malformed events),
//! plus wiremock request-shape tests through the real http-json transport.
//! Scope: fixture-based (in-test replay transport and local wiremock), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/tests/openai_common/mod.rs
//! External:
//! `-- wiremock::{Mock, MockServer, ResponseTemplate}
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations are copied from the baseline TS tests, not derived from Rust output.
//! Contract tests: provider-adapters/tests/openai_chat.rs

mod openai_common;

use std::sync::Arc;

use futures::StreamExt;
use openai_common::*;
use opencode_llm_api::{
    ContentPart, LlmClient, LlmEvent, LlmRequest, MediaData, Message, MessageRole, RouteClient,
    ToolCallPart, ToolContent, ToolResultPart, ToolResultValue,
};
use opencode_provider_adapters::auth::bearer_value;
use opencode_provider_adapters::protocols::openai::{
    chat_route, compatible_chat_route, sse_transport,
};
use opencode_provider_adapters::route::Transport;
use opencode_provider_adapters::transport::HttpExecutor;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn chat_model(transport: Arc<dyn Transport>) -> opencode_llm_api::Model {
    let mut route = chat_route(transport);
    route.endpoint.base_url = Some("https://api.openai.test/v1/".into());
    route.auth = bearer_value("test");
    model(route, "gpt-4o-mini", "openai")
}

fn base_request(transport: Arc<dyn Transport>) -> LlmRequest {
    let mut request = request(chat_model(transport));
    request.id = Some("req_1".into());
    request.system = system("You are concise.");
    request.messages = vec![Message::user("Say hello.")];
    request.generation = Some(generation(20.0, Some(0.0)));
    request
}

fn messages_only(messages: Vec<Message>) -> Value {
    let mut request = request(chat_model(Replay::new([])));
    request.messages = messages;
    prepare(request).unwrap()["messages"].clone()
}

fn tool_call(id: &str, name: &str, input: Value) -> ContentPart {
    ContentPart::ToolCall(ToolCallPart {
        id: id.into(),
        name: name.into(),
        input,
        provider_executed: None,
        metadata: None,
        provider_metadata: None,
    })
}

fn tool_result(id: &str, name: &str, result: ToolResultValue) -> ToolResultPart {
    ToolResultPart {
        id: id.into(),
        name: name.into(),
        result,
        provider_executed: None,
        cache: None,
        metadata: None,
        provider_metadata: None,
    }
}

fn file(uri: &str, mime: &str) -> ToolContent {
    ToolContent::File {
        uri: uri.into(),
        mime: mime.into(),
        name: None,
    }
}

fn media(media_type: &str, data: &str) -> ContentPart {
    ContentPart::Media {
        media_type: media_type.into(),
        data: MediaData::Text(data.into()),
        filename: None,
        metadata: None,
    }
}

fn delta_chunk(delta: Value, finish: Option<&str>) -> Value {
    json!({
        "id": "chatcmpl_fixture",
        "choices": [{ "delta": delta, "finish_reason": finish }],
        "usage": null,
    })
}

#[test]
fn prepares_openai_chat_payload() {
    let body = prepare(base_request(Replay::new([]))).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": "You are concise." },
                { "role": "user", "content": "Say hello." },
            ],
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": 20,
            "temperature": 0,
        })
    );
}

#[test]
fn lowers_chronological_system_updates() {
    assert_eq!(
        messages_only(vec![
            Message::user("Before."),
            Message::system("Treat <admin> & data literally."),
            Message::assistant(vec![ContentPart::text("After.")]),
        ]),
        json!([
            { "role": "user", "content": "Before.\n<system-update>\nTreat &lt;admin&gt; &amp; data literally.\n</system-update>" },
            { "role": "assistant", "content": "After." },
        ])
    );
}

#[test]
fn replays_reasoning_as_reasoning_content() {
    assert_eq!(
        messages_only(vec![Message::assistant(vec![
            ContentPart::reasoning("thinking"),
            ContentPart::text("Hello"),
        ])]),
        json!([{ "role": "assistant", "content": "Hello", "reasoning_content": "thinking" }])
    );
    assert_eq!(
        messages_only(vec![Message::assistant(vec![ContentPart::reasoning(
            "hidden"
        )])]),
        json!([{ "role": "assistant", "content": null, "reasoning_content": "hidden" }])
    );
    let mut native = Message::assistant(vec![ContentPart::text("x")]);
    native.native = Some(
        json!({ "openaiCompatible": { "reasoning_content": "native" } })
            .as_object()
            .unwrap()
            .clone(),
    );
    assert_eq!(
        messages_only(vec![native]),
        json!([{ "role": "assistant", "content": "x", "reasoning_content": "native" }])
    );
}

#[test]
fn maps_provider_options() {
    let mut request = base_request(Replay::new([]));
    request.provider_options =
        provider_options(json!({ "store": false, "reasoningEffort": "low" }));
    let body = prepare(request).unwrap();
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["reasoning_effort"], json!("low"));
    let mut request = base_request(Replay::new([]));
    request.provider_options = provider_options(json!({ "reasoningEffort": "max" }));
    assert_eq!(
        prepare(request).unwrap_err().reason.message(),
        "OpenAI Chat does not support reasoning effort max"
    );
    let mut request = base_request(Replay::new([]));
    request.provider_options = provider_options(json!({ "reasoningEffort": "bogus" }));
    assert!(prepare(request).unwrap().get("reasoning_effort").is_none());
}

#[test]
fn prepares_tool_call_and_tool_result_messages() {
    assert_eq!(
        messages_only(vec![
            Message::user("What is the weather?"),
            Message::assistant(vec![tool_call(
                "call_1",
                "lookup",
                json!({ "query": "weather" })
            )]),
            Message::tool(tool_result(
                "call_1",
                "lookup",
                ToolResultValue::Json(json!({ "forecast": "sunny" }))
            )),
        ]),
        json!([
            { "role": "user", "content": "What is the weather?" },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "lookup", "arguments": "{\"query\":\"weather\"}" } }],
            },
            { "role": "tool", "tool_call_id": "call_1", "content": "{\"forecast\":\"sunny\"}" },
        ])
    );
    let error = json!({ "error": { "type": "unknown", "message": "Tool execution interrupted" } });
    let messages = messages_only(vec![
        Message::assistant(vec![tool_call("call_1", "bash", json!({}))]),
        Message::tool(tool_result(
            "call_1",
            "bash",
            ToolResultValue::Error(error.clone()),
        )),
    ]);
    assert_eq!(
        messages[1],
        json!({ "role": "tool", "tool_call_id": "call_1", "content": error.to_string() })
    );
}

#[test]
fn image_tool_results_become_vision_messages() {
    let messages = messages_only(vec![
        Message::assistant(vec![tool_call(
            "call_image",
            "read",
            json!({ "path": "pixel.png" }),
        )]),
        Message::tool(tool_result(
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
        )),
    ]);
    assert_eq!(
        messages.as_array().unwrap()[1..],
        [
            json!({ "role": "tool", "tool_call_id": "call_image", "content": "Image read successfully" }),
            json!({ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,AAECAw==" } }] }),
        ]
    );
    // Parallel tool responses come before one aggregated vision message.
    let messages = messages_only(vec![
        Message::assistant(vec![
            tool_call("call_1", "read", json!({})),
            tool_call("call_2", "read", json!({})),
        ]),
        Message::new(
            MessageRole::Tool,
            vec![
                ContentPart::ToolResult(tool_result(
                    "call_1",
                    "read",
                    ToolResultValue::Content(vec![file("data:image/png;base64,AAEC", "image/png")]),
                )),
                ContentPart::ToolResult(tool_result(
                    "call_2",
                    "read",
                    ToolResultValue::Content(vec![file(
                        "data:image/jpeg;base64,/9j/",
                        "image/jpeg",
                    )]),
                )),
            ],
        ),
    ]);
    assert_eq!(
        messages.as_array().unwrap()[1..],
        [
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "" }),
            json!({ "role": "tool", "tool_call_id": "call_2", "content": "" }),
            json!({ "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAEC" } },
                { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,/9j/" } },
            ] }),
        ]
    );
    // Consecutive tool images merge with a following system update.
    assert_eq!(
        messages_only(vec![
            Message::tool(tool_result(
                "call_1",
                "read",
                ToolResultValue::Content(vec![file("data:image/png;base64,AAEC", "image/png")])
            )),
            Message::tool(tool_result(
                "call_2",
                "read",
                ToolResultValue::Content(vec![file("data:image/webp;base64,UklG", "image/webp")])
            )),
            Message::system("Inspect both images."),
        ]),
        json!([
            { "role": "tool", "tool_call_id": "call_1", "content": "" },
            { "role": "tool", "tool_call_id": "call_2", "content": "" },
            { "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAEC" } },
                { "type": "image_url", "image_url": { "url": "data:image/webp;base64,UklG" } },
                { "type": "text", "text": "<system-update>\nInspect both images.\n</system-update>" },
            ] },
        ])
    );
}

#[test]
fn user_media_lowering_and_validation() {
    assert_eq!(
        messages_only(vec![
            Message::new(MessageRole::User, vec![media("image/png", "AAEC")]),
            Message::system("Keep the image."),
        ]),
        json!([{ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAEC" } },
            { "type": "text", "text": "<system-update>\nKeep the image.\n</system-update>" },
        ] }])
    );
    assert_eq!(
        messages_only(vec![Message::new(
            MessageRole::User,
            vec![
                media("image/png", "AAECAw=="),
                media("image/jpeg", "data:image/jpeg;base64,/9j/")
            ],
        )]),
        json!([{ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAECAw==" } },
            { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,/9j/" } },
        ] }])
    );
    for (media_type, data) in [
        ("image/png", "data:image/jpeg;base64,/9j/"),
        ("image/png", "not-base64"),
        ("image/svg+xml", "PHN2Zz4="),
    ] {
        let mut request = request(chat_model(Replay::new([])));
        request.messages = vec![Message::new(
            MessageRole::User,
            vec![media(media_type, data)],
        )];
        let message = prepare(request).unwrap_err().reason.message();
        assert!(
            ["does not support", "does not match", "valid base64"]
                .iter()
                .any(|m| message.contains(m)),
            "{message}"
        );
    }
    let mut request = request(chat_model(Replay::new([])));
    request.messages = vec![Message::new(
        MessageRole::User,
        vec![media("image/png", &"A".repeat(28 * 1024 * 1024 + 4))],
    )];
    assert!(
        prepare(request)
            .unwrap_err()
            .reason
            .message()
            .contains("encoded limit")
    );
}

#[tokio::test]
async fn parses_text_and_usage() {
    let usage = json!({
        "prompt_tokens": 5,
        "completion_tokens": 2,
        "total_tokens": 7,
        "prompt_tokens_details": { "cached_tokens": 1 },
        "completion_tokens_details": { "reasoning_tokens": 0 },
    });
    let body = sse(&[
        delta_chunk(json!({ "role": "assistant", "content": "Hello" }), None),
        delta_chunk(json!({ "content": "!" }), None),
        delta_chunk(json!({}), Some("stop")),
        json!({ "id": "chatcmpl_fixture", "choices": [], "usage": usage }),
    ]);
    let response = generate(base_request(Replay::new([body]))).await.unwrap();
    let usage_json = json!({
        "inputTokens": 5, "outputTokens": 2, "nonCachedInputTokens": 4, "cacheReadInputTokens": 1,
        "reasoningTokens": 0, "totalTokens": 7, "providerMetadata": { "openai": usage },
    });
    assert_eq!(response.text(), "Hello!");
    assert_eq!(
        to_json(&response.events),
        [
            json!({ "type": "step-start", "index": 0 }),
            json!({ "type": "text-start", "id": "text-0" }),
            json!({ "type": "text-delta", "id": "text-0", "text": "Hello" }),
            json!({ "type": "text-delta", "id": "text-0", "text": "!" }),
            json!({ "type": "text-end", "id": "text-0" }),
            json!({ "type": "step-finish", "index": 0, "reason": "stop", "usage": usage_json }),
            json!({ "type": "finish", "reason": "stop", "usage": usage_json }),
        ]
    );
}

#[tokio::test]
async fn parses_reasoning_content_deltas() {
    let body = sse(&[
        json!({ "choices": [{ "delta": { "reasoning_content": "thinking" } }] }),
        json!({ "choices": [{ "delta": { "content": "Hello" } }] }),
        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
    ]);
    let response = generate(base_request(Replay::new([body]))).await.unwrap();
    assert_eq!(response.reasoning(), "thinking");
    assert_eq!(
        kinds(&response.events),
        [
            "step-start",
            "reasoning-start",
            "reasoning-delta",
            "reasoning-end",
            "text-start",
            "text-delta",
            "text-end",
            "step-finish",
            "finish"
        ]
    );
}

#[tokio::test]
async fn assembles_streamed_tool_call_input() {
    let body = sse(&[
        delta_chunk(
            json!({ "role": "assistant", "tool_calls": [{ "index": 0, "id": "call_1", "function": { "name": "lookup", "arguments": "{\"query\"" } }] }),
            None,
        ),
        delta_chunk(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": ":\"weather\"}" } }] }),
            None,
        ),
        delta_chunk(json!({}), Some("tool_calls")),
    ]);
    let response = generate(base_request(Replay::new([body]))).await.unwrap();
    assert_eq!(
        to_json(&response.events),
        [
            json!({ "type": "step-start", "index": 0 }),
            json!({ "type": "tool-input-start", "id": "call_1", "name": "lookup" }),
            json!({ "type": "tool-input-delta", "id": "call_1", "name": "lookup", "text": "{\"query\"" }),
            json!({ "type": "tool-input-delta", "id": "call_1", "name": "lookup", "text": ":\"weather\"}" }),
            json!({ "type": "tool-input-end", "id": "call_1", "name": "lookup" }),
            json!({ "type": "tool-call", "id": "call_1", "name": "lookup", "input": { "query": "weather" } }),
            json!({ "type": "step-finish", "index": 0, "reason": "tool-calls" }),
            json!({ "type": "finish", "reason": "tool-calls" }),
        ]
    );
}

#[tokio::test]
async fn does_not_finalize_tool_calls_without_finish_reason() {
    let body = sse(&[
        delta_chunk(
            json!({ "role": "assistant", "tool_calls": [{ "index": 0, "id": "call_1", "function": { "name": "lookup", "arguments": "{\"query\"" } }] }),
            None,
        ),
        delta_chunk(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": ":\"weather\"}" } }] }),
            None,
        ),
    ]);
    let request = base_request(Replay::new([body.clone()]));
    let events: Vec<LlmEvent> = RouteClient
        .stream(request)
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(
        kinds(&events),
        [
            "step-start",
            "tool-input-start",
            "tool-input-delta",
            "tool-input-delta"
        ]
    );
    let error = generate(base_request(Replay::new([body])))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Provider stream ended without a terminal finish event")
    );
}

#[tokio::test]
async fn fails_on_malformed_stream_events() {
    let body = sse(&[delta_chunk(json!({ "content": 123 }), None)]);
    let error = generate(base_request(Replay::new([body])))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid openai/openai-chat stream event"),
        "{error}"
    );
    let body = sse(&[
        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "{}" } }] } }] }),
    ]);
    let error = generate(base_request(Replay::new([body])))
        .await
        .unwrap_err();
    assert_eq!(
        error.reason.message(),
        "OpenAI Chat tool call delta is missing id or name"
    );
}

#[tokio::test]
async fn invalid_tool_json_fails_at_finish_reason() {
    let body = sse(&[
        delta_chunk(
            json!({ "tool_calls": [{ "index": 0, "id": "c", "function": { "name": "f", "arguments": "{bad" } }] }),
            None,
        ),
        delta_chunk(json!({}), Some("tool_calls")),
    ]);
    let error = generate(base_request(Replay::new([body])))
        .await
        .unwrap_err();
    assert_eq!(
        error.reason.message(),
        "Invalid JSON input for openai-chat tool call f"
    );
}

#[tokio::test]
async fn finish_reason_mapping() {
    for (reason, expected) in [
        ("length", "length"),
        ("content_filter", "content-filter"),
        ("function_call", "tool-calls"),
        ("weird", "unknown"),
    ] {
        let body = sse(&[delta_chunk(json!({ "content": "x" }), Some(reason))]);
        let response = generate(base_request(Replay::new([body]))).await.unwrap();
        assert_eq!(
            serde_json::to_value(response.finish_reason).unwrap(),
            json!(expected)
        );
    }
}

#[tokio::test]
async fn http_request_shape_and_overlays_over_wiremock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(query_param("debug", "1"))
        .and(header("authorization", "Bearer fresh-key"))
        .and(header("x-custom", "yes"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": "You are concise." },
                { "role": "user", "content": "Say hello." },
            ],
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": 20,
            "temperature": 0,
            "metadata": { "source": "test" },
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&[delta_chunk(
                    json!({ "content": "Hi" }),
                    Some("stop"),
                )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut route = chat_route(sse_transport(HttpExecutor::new().unwrap()));
    route.endpoint.base_url = Some(format!("{}/v1/", server.uri()));
    route.auth = bearer_value("fresh-key");
    let mut request = request(model(route, "gpt-4o-mini", "openai"));
    request.system = system("You are concise.");
    request.messages = vec![Message::user("Say hello.")];
    request.generation = Some(generation(20.0, Some(0.0)));
    request.http = Some(opencode_llm_api::HttpOptions {
        body: Some(
            json!({ "metadata": { "source": "test" } })
                .as_object()
                .unwrap()
                .clone(),
        ),
        headers: Some(
            [
                ("authorization".to_owned(), "Bearer request".to_owned()),
                ("x-custom".to_owned(), "yes".to_owned()),
            ]
            .into_iter()
            .collect(),
        ),
        query: Some([("debug".to_owned(), "1".to_owned())].into_iter().collect()),
    });
    let response = generate(request).await.unwrap();
    assert_eq!(response.text(), "Hi");
}

#[tokio::test]
async fn http_status_errors_fail_before_stream_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"error":{"message":"Bad request","type":"invalid_request_error"}}"#,
                ),
        )
        .mount(&server)
        .await;
    let mut route = compatible_chat_route(sse_transport(HttpExecutor::new().unwrap()));
    route.endpoint.base_url = Some(format!("{}/v1", server.uri()));
    let mut request = request(model(route, "m", "groq"));
    request.messages = vec![Message::user("hi")];
    let error = generate(request).await.unwrap_err();
    assert_eq!(error.reason.tag(), "InvalidRequest");
    assert!(error.to_string().contains("HTTP 400"));
    assert_eq!(
        server.received_requests().await.unwrap()[0].url.path(),
        "/v1/chat/completions"
    );
}

#[tokio::test]
async fn consumer_prefix_drops_the_rest() {
    let body = sse(&[
        delta_chunk(json!({ "role": "assistant", "content": "Hello" }), None),
        delta_chunk(json!({ "content": " world" }), None),
        delta_chunk(json!({}), Some("stop")),
    ]);
    let events: Vec<_> = RouteClient
        .stream(base_request(Replay::new([body])))
        .take(1)
        .collect()
        .await;
    assert_eq!(
        kinds(&events.into_iter().map(Result::unwrap).collect::<Vec<_>>()),
        ["step-start"]
    );
}
