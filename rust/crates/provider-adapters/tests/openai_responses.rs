//! Purpose: OpenAI Responses protocol behavior tests ported from
//! packages/llm/test/provider/openai-responses.test.ts: payload lowering (system updates,
//! service tier, options/include filtering, tool calls/outputs incl. images and errors, reasoning
//! replay with store true/false, hosted tool references, media), stream decoding (text/usage,
//! reasoning summaries and parts, encrypted state, function calls, hosted tools, provider errors
//! with context-overflow), WebSocket mode over a local tungstenite server, and HTTP errors.
//! Scope: fixture-based (in-test replay transport, local wiremock and WebSocket servers).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/tests/openai_common/mod.rs
//! External:
//! |-- futures::SinkExt
//! |-- futures::StreamExt
//! |-- pretty_assertions::assert_eq
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio::time::timeout
//! |-- tokio_tungstenite::accept_hdr_async
//! |-- tokio_tungstenite::tungstenite::Message
//! |-- tokio_tungstenite::tungstenite::handshake::server::Request
//! |-- tokio_tungstenite::tungstenite::handshake::server::Response
//! |-- wiremock::Mock
//! |-- wiremock::MockServer
//! |-- wiremock::ResponseTemplate
//! `-- wiremock::matchers::method
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations are copied from the baseline TS tests, not derived from Rust output.
//! Contract tests: provider-adapters/tests/openai_responses.rs

mod openai_common;

use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use openai_common::*;
use opencode_llm_api::{
    ContentPart, LlmEvent, LlmRequest, MediaData, Message, MessageRole, ToolCallPart, ToolChoice,
    ToolContent, ToolResultPart, ToolResultValue,
};
use opencode_provider_adapters::auth::bearer_value;
use opencode_provider_adapters::protocols::openai::{
    responses_route, responses_websocket_route, sse_transport,
};
use opencode_provider_adapters::route::Transport;
use opencode_provider_adapters::transport::HttpExecutor;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn responses_model(transport: Arc<dyn Transport>) -> opencode_llm_api::Model {
    let mut route = responses_route(transport);
    route.endpoint.base_url = Some("https://api.openai.test/v1/".into());
    route.auth = bearer_value("test");
    model(route, "gpt-4.1-mini", "openai")
}

fn base_request(transport: Arc<dyn Transport>) -> LlmRequest {
    let mut request = request(responses_model(transport));
    request.id = Some("req_1".into());
    request.system = system("You are concise.");
    request.messages = vec![Message::user("Say hello.")];
    request.generation = Some(generation(20.0, Some(0.0)));
    request
}

fn body_for(configure: impl FnOnce(&mut LlmRequest)) -> Value {
    let mut request = request(responses_model(Replay::new([])));
    configure(&mut request);
    prepare(request).unwrap()
}

fn reasoning(text: &str, metadata: Value) -> ContentPart {
    ContentPart::Reasoning {
        text: text.into(),
        encrypted: None,
        metadata: None,
        provider_metadata: Some(
            [("openai".to_owned(), metadata.as_object().unwrap().clone())]
                .into_iter()
                .collect(),
        ),
    }
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

async fn events_for(stream: &[Value], configure: impl FnOnce(&mut LlmRequest)) -> Vec<LlmEvent> {
    let mut request = base_request(Replay::new([sse(stream)]));
    configure(&mut request);
    generate(request).await.unwrap().events
}

#[test]
fn prepares_target_and_options() {
    assert_eq!(
        prepare(base_request(Replay::new([]))).unwrap(),
        json!({
            "model": "gpt-4.1-mini",
            "input": [
                { "role": "system", "content": "You are concise." },
                { "role": "user", "content": [{ "type": "input_text", "text": "Say hello." }] },
            ],
            "store": false,
            "stream": true,
            "max_output_tokens": 20,
            "temperature": 0,
        })
    );
    let body = body_for(|r| {
        r.messages = vec![Message::user("hi")];
        r.provider_options = provider_options(json!({ "serviceTier": "priority" }));
    });
    assert_eq!(body["service_tier"], json!("priority"));
    let body =
        body_for(|r| r.provider_options = provider_options(json!({ "serviceTier": "scale" })));
    assert!(body.get("service_tier").is_none());
    let body = body_for(|r| {
        r.provider_options = provider_options(json!({
            "promptCacheKey": "session_123",
            "reasoningEffort": "high",
            "reasoningSummary": "auto",
            "include": ["reasoning.encrypted_content", "bogus.thing", "code_interpreter_call.outputs"],
            "textVerbosity": "low",
            "instructions": "Be brief.",
        }))
    });
    assert_eq!(body["prompt_cache_key"], json!("session_123"));
    assert_eq!(
        body["include"],
        json!([
            "reasoning.encrypted_content",
            "code_interpreter_call.outputs"
        ])
    );
    assert_eq!(
        body["reasoning"],
        json!({ "effort": "high", "summary": "auto" })
    );
    assert_eq!(body["text"], json!({ "verbosity": "low" }));
    assert_eq!(body["instructions"], json!("Be brief."));
    for include in [json!([]), json!(["bogus.thing"])] {
        let body =
            body_for(|r| r.provider_options = provider_options(json!({ "include": include })));
        assert!(body.get("include").is_none());
    }
    let mut request = base_request(Replay::new([]));
    request.provider_options = provider_options(json!({ "reasoningEffort": "max" }));
    assert_eq!(
        prepare(request).unwrap_err().reason.message(),
        "OpenAI Responses does not support reasoning effort max"
    );
    let body =
        body_for(|r| r.provider_options = provider_options(json!({ "reasoningSummary": "auto" })));
    assert_eq!(body["reasoning"], json!({ "summary": "auto" }));
}

#[test]
fn request_options_override_route_defaults() {
    let body = body_for(|r| r.provider_options = provider_options(json!({ "store": true })));
    assert_eq!(body["store"], json!(true));
}

#[test]
fn flattens_top_level_object_unions_and_tool_choice() {
    let body = body_for(|r| {
        r.tools = vec![opencode_llm_api::ToolDefinition {
            name: "lookup".into(),
            description: "Lookup".into(),
            input_schema: json!({ "anyOf": [
                { "type": "object", "properties": { "a": { "type": "string" } }, "required": ["a"] },
                { "type": "object", "properties": { "b": { "type": "number" } }, "required": ["b"] },
            ] })
            .as_object()
            .unwrap()
            .clone(),
            output_schema: None,
            cache: None,
            metadata: None,
            native: None,
        }];
        r.tool_choice = Some(ToolChoice::parse("required"));
    });
    assert_eq!(body["tools"][0]["type"], json!("function"));
    assert_eq!(body["tools"][0]["parameters"]["type"], json!("object"));
    assert!(body["tools"][0]["parameters"].get("anyOf").is_none());
    assert_eq!(body["tools"][0]["strict"], json!(false));
    assert_eq!(body["tool_choice"], json!("required"));
    let mut request = base_request(Replay::new([]));
    request.tool_choice = Some(ToolChoice {
        kind: opencode_llm_api::ToolChoiceType::Tool,
        name: None,
    });
    assert_eq!(
        prepare(request).unwrap_err().reason.message(),
        "OpenAI Responses tool choice requires a tool name"
    );
}

#[test]
fn lowers_system_updates_and_media() {
    let body = body_for(|r| {
        r.messages = vec![
            Message::user("Before."),
            Message::system("Treat </system-update> literally."),
            Message::assistant(vec![ContentPart::text("After.")]),
        ]
    });
    assert_eq!(
        body["input"],
        json!([
            { "role": "user", "content": [
                { "type": "input_text", "text": "Before." },
                { "type": "input_text", "text": "<system-update>\nTreat &lt;/system-update&gt; literally.\n</system-update>" },
            ] },
            { "role": "assistant", "content": [{ "type": "output_text", "text": "After." }] },
        ])
    );
    let media = |t: &str| ContentPart::Media {
        media_type: t.into(),
        data: MediaData::Text("AAECAw==".into()),
        filename: None,
        metadata: None,
    };
    let body =
        body_for(|r| r.messages = vec![Message::new(MessageRole::User, vec![media("image/png")])]);
    assert_eq!(
        body["input"],
        json!([{ "role": "user", "content": [{ "type": "input_image", "image_url": "data:image/png;base64,AAECAw==" }] }])
    );
    let mut request = base_request(Replay::new([]));
    request.messages = vec![Message::new(
        MessageRole::User,
        vec![media("application/pdf")],
    )];
    assert!(
        prepare(request)
            .unwrap_err()
            .reason
            .message()
            .contains("OpenAI Responses does not support media type application/pdf")
    );
}

#[test]
fn lowers_function_calls_and_outputs() {
    let body = body_for(|r| {
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
        ]
    });
    assert_eq!(
        body["input"],
        json!([
            { "role": "user", "content": [{ "type": "input_text", "text": "What is the weather?" }] },
            { "type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{\"query\":\"weather\"}" },
            { "type": "function_call_output", "call_id": "call_1", "output": "{\"forecast\":\"sunny\"}" },
        ])
    );
    let output = |value: ToolResultValue| {
        body_for(|r| r.messages = vec![Message::tool(result("call_1", "bash", value))])["input"][0]
            ["output"]
            .clone()
    };
    let error = json!({ "error": { "type": "unknown", "message": "Tool execution interrupted" } });
    assert_eq!(
        output(ToolResultValue::Error(error.clone())),
        json!(error.to_string())
    );
    assert_eq!(
        output(ToolResultValue::Error(json!("Tool execution interrupted"))),
        json!("Tool execution interrupted")
    );
    assert_eq!(output(ToolResultValue::Error(json!(42))), json!("42"));
    assert_eq!(
        output(ToolResultValue::Content(vec![
            ToolContent::Text {
                text: "Image read successfully".into()
            },
            ToolContent::File {
                uri: "data:image/png;base64,AAECAw==".into(),
                mime: "image/png".into(),
                name: None
            },
        ])),
        json!([
            { "type": "input_text", "text": "Image read successfully" },
            { "type": "input_image", "image_url": "data:image/png;base64,AAECAw==" },
        ])
    );
    let mut request = base_request(Replay::new([]));
    request.messages = vec![Message::tool(result(
        "c",
        "read",
        ToolResultValue::Content(vec![ToolContent::File {
            uri: "data:audio/mpeg;base64,AAECAw==".into(),
            mime: "audio/mpeg".into(),
            name: None,
        }]),
    ))];
    let message = prepare(request).unwrap_err().reason.message();
    assert!(
        message.contains("OpenAI Responses") && message.contains("audio/mpeg"),
        "{message}"
    );
}

#[test]
fn reasoning_replay_rules() {
    // store:false keeps encrypted state, order around text, joins blocks per item, drops no-state items.
    let store_false = || provider_options(json!({ "store": false }));
    let body = body_for(|r| {
        r.provider_options = store_false();
        r.messages = vec![Message::assistant(vec![
            ContentPart::text("Before."),
            reasoning(
                "Checked order.",
                json!({ "itemId": "rs_1", "reasoningEncryptedContent": "encrypted-state" }),
            ),
            ContentPart::text("After."),
        ])];
    });
    assert_eq!(
        body["input"],
        json!([
            { "role": "assistant", "content": [{ "type": "output_text", "text": "Before." }] },
            { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "Checked order." }], "encrypted_content": "encrypted-state" },
            { "role": "assistant", "content": [{ "type": "output_text", "text": "After." }] },
        ])
    );
    let body = body_for(|r| {
        r.provider_options = store_false();
        r.messages = vec![Message::assistant(vec![
            reasoning("First", json!({ "itemId": "rs_1" })),
            reasoning(
                "Second",
                json!({ "itemId": "rs_1", "reasoningEncryptedContent": "encrypted-state" }),
            ),
        ])];
    });
    assert_eq!(
        body["input"],
        json!([{ "type": "reasoning", "summary": [
            { "type": "summary_text", "text": "First" },
            { "type": "summary_text", "text": "Second" },
        ], "encrypted_content": "encrypted-state" }])
    );
    let body = body_for(|r| {
        r.provider_options = store_false();
        r.messages = vec![
            Message::user("What changed?"),
            Message::assistant(vec![
                reasoning(
                    "Checked the previous diff.",
                    json!({ "itemId": "rs_1", "reasoningEncryptedContent": null }),
                ),
                ContentPart::text("The parser changed."),
            ]),
            Message::user("Summarize it."),
        ];
    });
    assert_eq!(
        body["input"],
        json!([
            { "role": "user", "content": [{ "type": "input_text", "text": "What changed?" }] },
            { "role": "assistant", "content": [{ "type": "output_text", "text": "The parser changed." }] },
            { "role": "user", "content": [{ "type": "input_text", "text": "Summarize it." }] },
        ])
    );
    // store:true references reasoning items and hosted tool results by id.
    let body = body_for(|r| {
        r.provider_options = provider_options(json!({ "store": true }));
        r.messages = vec![Message::assistant(vec![reasoning(
            "x",
            json!({ "itemId": "rs_1" }),
        )])];
    });
    assert_eq!(
        body["input"],
        json!([{ "type": "item_reference", "id": "rs_1" }])
    );
    let hosted_meta: opencode_llm_api::ProviderMetadata = [(
        "openai".to_owned(),
        json!({ "itemId": "ws_1" }).as_object().unwrap().clone(),
    )]
    .into_iter()
    .collect();
    let body = body_for(|r| {
        r.provider_options = provider_options(json!({ "store": true }));
        r.messages = vec![
            Message::assistant(vec![
                ContentPart::ToolCall(ToolCallPart {
                    id: "ws_1".into(),
                    name: "web_search".into(),
                    input: json!({ "query": "effect 4" }),
                    provider_executed: Some(true),
                    metadata: None,
                    provider_metadata: Some(hosted_meta.clone()),
                }),
                ContentPart::ToolResult(ToolResultPart {
                    provider_executed: Some(true),
                    provider_metadata: Some(hosted_meta.clone()),
                    ..result(
                        "ws_1",
                        "web_search",
                        ToolResultValue::Json(
                            json!({ "type": "web_search_call", "id": "ws_1", "status": "completed" }),
                        ),
                    )
                }),
            ]),
            Message::user("Continue."),
        ];
    });
    assert_eq!(
        body["input"],
        json!([
            { "type": "item_reference", "id": "ws_1" },
            { "role": "user", "content": [{ "type": "input_text", "text": "Continue." }] },
        ])
    );
}

#[tokio::test]
async fn parses_text_and_usage() {
    let usage = json!({
        "input_tokens": 5, "output_tokens": 2, "total_tokens": 7,
        "input_tokens_details": { "cached_tokens": 1 },
        "output_tokens_details": { "reasoning_tokens": 0 },
    });
    let events = events_for(
        &[
            json!({ "type": "response.output_text.delta", "item_id": "msg_1", "delta": "Hello" }),
            json!({ "type": "response.output_text.delta", "item_id": "msg_1", "delta": "!" }),
            json!({ "type": "response.completed", "response": { "id": "resp_1", "service_tier": "default", "usage": usage } }),
        ],
        |_| {},
    )
    .await;
    let usage_json = json!({
        "inputTokens": 5, "outputTokens": 2, "nonCachedInputTokens": 4, "cacheReadInputTokens": 1,
        "reasoningTokens": 0, "totalTokens": 7,
        "providerMetadata": { "openai": {
            "input_tokens": 5, "input_tokens_details": { "cached_tokens": 1 }, "output_tokens": 2,
            "output_tokens_details": { "reasoning_tokens": 0 }, "total_tokens": 7,
        } },
    });
    let meta = json!({ "openai": { "responseId": "resp_1", "serviceTier": "default" } });
    assert_eq!(
        to_json(&events),
        [
            json!({ "type": "step-start", "index": 0 }),
            json!({ "type": "text-start", "id": "msg_1" }),
            json!({ "type": "text-delta", "id": "msg_1", "text": "Hello" }),
            json!({ "type": "text-delta", "id": "msg_1", "text": "!" }),
            json!({ "type": "text-end", "id": "msg_1" }),
            json!({ "type": "step-finish", "index": 0, "reason": "stop", "usage": usage_json, "providerMetadata": meta }),
            json!({ "type": "finish", "reason": "stop", "usage": usage_json, "providerMetadata": meta }),
        ]
    );
}

#[tokio::test]
async fn parses_reasoning_summaries_and_encrypted_state() {
    let events = events_for(
        &[
            json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "delta": "thinking" }),
            json!({ "type": "response.output_text.delta", "item_id": "msg_1", "delta": "Hello" }),
            json!({ "type": "response.reasoning_summary_text.done", "item_id": "rs_1" }),
            json!({ "type": "response.completed", "response": { "id": "resp_1" } }),
        ],
        |_| {},
    )
    .await;
    assert_eq!(
        kinds(&events),
        [
            "step-start",
            "reasoning-start",
            "reasoning-delta",
            "text-start",
            "text-delta",
            "reasoning-end",
            "text-end",
            "step-finish",
            "finish"
        ]
    );
    let events = events_for(
        &[
            json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "delta": "thinking" }),
            json!({ "type": "response.output_item.done", "item": { "type": "reasoning", "id": "rs_1", "encrypted_content": "encrypted-state", "summary": [] } }),
            json!({ "type": "response.completed", "response": { "id": "resp_1" } }),
        ],
        |_| {},
    )
    .await;
    assert!(to_json(&events).contains(&json!({
        "type": "reasoning-end",
        "id": "rs_1",
        "providerMetadata": { "openai": { "itemId": "rs_1", "reasoningEncryptedContent": "encrypted-state" } },
    })));
}

fn summary_parts(encrypted_done: Value) -> Vec<Value> {
    vec![
        json!({ "type": "response.output_item.added", "item": { "type": "reasoning", "id": "rs_1", "encrypted_content": null } }),
        json!({ "type": "response.reasoning_summary_part.added", "item_id": "rs_1", "summary_index": 0 }),
        json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "summary_index": 0, "delta": "First" }),
        json!({ "type": "response.reasoning_summary_part.done", "item_id": "rs_1", "summary_index": 0 }),
        json!({ "type": "response.reasoning_summary_part.added", "item_id": "rs_1", "summary_index": 1 }),
        json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "summary_index": 1, "delta": "Second" }),
        json!({ "type": "response.reasoning_summary_part.done", "item_id": "rs_1", "summary_index": 1 }),
        json!({ "type": "response.output_item.done", "item": { "type": "reasoning", "id": "rs_1", "encrypted_content": encrypted_done } }),
        json!({ "type": "response.completed", "response": { "id": "resp_1" } }),
    ]
}

#[tokio::test]
async fn reasoning_summary_parts_store_false_and_true() {
    let events = events_for(&summary_parts(json!("encrypted-state")), |r| {
        r.provider_options = provider_options(json!({ "store": false }))
    })
    .await;
    let null_meta = json!({ "openai": { "itemId": "rs_1", "reasoningEncryptedContent": null } });
    assert_eq!(
        to_json(&events)[..7],
        [
            json!({ "type": "step-start", "index": 0 }),
            json!({ "type": "reasoning-start", "id": "rs_1:0", "providerMetadata": null_meta }),
            json!({ "type": "reasoning-delta", "id": "rs_1:0", "text": "First" }),
            json!({ "type": "reasoning-end", "id": "rs_1:0", "providerMetadata": { "openai": { "itemId": "rs_1" } } }),
            json!({ "type": "reasoning-start", "id": "rs_1:1", "providerMetadata": null_meta }),
            json!({ "type": "reasoning-delta", "id": "rs_1:1", "text": "Second" }),
            json!({ "type": "reasoning-end", "id": "rs_1:1", "providerMetadata": { "openai": { "itemId": "rs_1", "reasoningEncryptedContent": "encrypted-state" } } }),
        ]
    );
    let events = events_for(&summary_parts(Value::Null), |r| {
        r.provider_options = provider_options(json!({ "store": true }))
    })
    .await;
    let ends: Vec<Value> = to_json(&events)
        .into_iter()
        .filter(|e| e["type"] == "reasoning-end")
        .collect();
    assert_eq!(
        ends,
        [
            json!({ "type": "reasoning-end", "id": "rs_1:0", "providerMetadata": { "openai": { "itemId": "rs_1" } } }),
            json!({ "type": "reasoning-end", "id": "rs_1:1", "providerMetadata": { "openai": { "itemId": "rs_1" } } }),
        ]
    );
}

#[tokio::test]
async fn assembles_function_call_input() {
    let events = events_for(
        &[
            json!({ "type": "response.output_item.added", "item": { "type": "function_call", "id": "item_1", "call_id": "call_1", "name": "lookup", "arguments": "" } }),
            json!({ "type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": "{\"query\"" }),
            json!({ "type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": ":\"weather\"}" }),
            json!({ "type": "response.output_item.done", "item": { "type": "function_call", "id": "item_1", "call_id": "call_1", "name": "lookup", "arguments": "{\"query\":\"weather\"}" } }),
            json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 5, "output_tokens": 1 } } }),
        ],
        |_| {},
    )
    .await;
    let meta = json!({ "openai": { "itemId": "item_1" } });
    let usage = json!({
        "inputTokens": 5, "outputTokens": 1, "nonCachedInputTokens": 5, "totalTokens": 6,
        "providerMetadata": { "openai": { "input_tokens": 5, "output_tokens": 1 } },
    });
    assert_eq!(
        to_json(&events),
        [
            json!({ "type": "step-start", "index": 0 }),
            json!({ "type": "tool-input-start", "id": "call_1", "name": "lookup", "providerMetadata": meta }),
            json!({ "type": "tool-input-delta", "id": "call_1", "name": "lookup", "text": "{\"query\"" }),
            json!({ "type": "tool-input-delta", "id": "call_1", "name": "lookup", "text": ":\"weather\"}" }),
            json!({ "type": "tool-input-end", "id": "call_1", "name": "lookup", "providerMetadata": meta }),
            json!({ "type": "tool-call", "id": "call_1", "name": "lookup", "input": { "query": "weather" }, "providerMetadata": meta }),
            json!({ "type": "step-finish", "index": 0, "reason": "tool-calls", "usage": usage }),
            json!({ "type": "finish", "reason": "tool-calls", "usage": usage }),
        ]
    );
}

#[tokio::test]
async fn decodes_hosted_tools() {
    let item = json!({ "type": "web_search_call", "id": "ws_1", "status": "completed", "action": { "type": "search", "query": "effect 4" } });
    let events = events_for(
        &[
            json!({ "type": "response.output_item.added", "item": item }),
            json!({ "type": "response.output_item.done", "item": item }),
            json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 5, "output_tokens": 1 } } }),
        ],
        |_| {},
    )
    .await;
    let meta = json!({ "openai": { "itemId": "ws_1" } });
    let pairs: Vec<Value> = to_json(&events)
        .into_iter()
        .filter(|e| e["type"] == "tool-call" || e["type"] == "tool-result")
        .collect();
    assert_eq!(
        pairs,
        [
            json!({ "type": "tool-call", "id": "ws_1", "name": "web_search", "input": { "type": "search", "query": "effect 4" }, "providerExecuted": true, "providerMetadata": meta }),
            json!({ "type": "tool-result", "id": "ws_1", "name": "web_search", "result": { "type": "json", "value": item }, "providerExecuted": true, "providerMetadata": meta }),
        ]
    );
    let item = json!({ "type": "code_interpreter_call", "id": "ci_1", "status": "completed", "code": "print(1+1)", "container_id": "cnt_xyz", "outputs": [{ "type": "logs", "logs": "2\n" }] });
    let events = to_json(
        &events_for(
            &[
                json!({ "type": "response.output_item.done", "item": item }),
                json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 5, "output_tokens": 1 } } }),
            ],
            |_| {},
        )
        .await,
    );
    assert!(events.contains(&json!({ "type": "tool-call", "id": "ci_1", "name": "code_interpreter", "input": { "code": "print(1+1)", "container_id": "cnt_xyz" }, "providerExecuted": true, "providerMetadata": { "openai": { "itemId": "ci_1" } } })));
    assert!(events.contains(&json!({ "type": "tool-result", "id": "ci_1", "name": "code_interpreter", "result": { "type": "json", "value": item }, "providerExecuted": true, "providerMetadata": { "openai": { "itemId": "ci_1" } } })));
    let failed = json!({ "type": "mcp_call", "id": "mcp_1", "server_label": "s", "name": "n", "arguments": "{}", "error": "boom" });
    let events = to_json(
        &events_for(
            &[
                json!({ "type": "response.output_item.done", "item": failed }),
                json!({ "type": "response.completed", "response": {} }),
            ],
            |_| {},
        )
        .await,
    );
    assert!(events.contains(&json!({ "type": "tool-result", "id": "mcp_1", "name": "mcp", "result": { "type": "error", "value": "boom" }, "providerExecuted": true, "providerMetadata": { "openai": { "itemId": "mcp_1" } } })));
}

#[tokio::test]
async fn provider_error_events() {
    let cases = [
        (
            json!({ "type": "error", "code": "rate_limit_exceeded", "message": "Slow down" }),
            json!({ "type": "provider-error", "message": "rate_limit_exceeded: Slow down" }),
        ),
        (
            json!({ "type": "error", "code": "internal_error" }),
            json!({ "type": "provider-error", "message": "internal_error" }),
        ),
        (
            json!({ "type": "error", "code": "internal_error", "message": "" }),
            json!({ "type": "provider-error", "message": "internal_error" }),
        ),
        (
            json!({ "type": "response.failed", "response": { "id": "r", "error": { "code": "server_error", "message": "Upstream model unavailable" } } }),
            json!({ "type": "provider-error", "message": "server_error: Upstream model unavailable" }),
        ),
        (
            json!({ "type": "response.failed", "response": { "id": "r", "error": { "code": "invalid_prompt" } } }),
            json!({ "type": "provider-error", "message": "invalid_prompt" }),
        ),
        (
            json!({ "type": "error", "response": { "error": { "code": "context_length_exceeded", "message": "prompt too long" } } }),
            json!({ "type": "provider-error", "message": "context_length_exceeded: prompt too long", "classification": "context-overflow" }),
        ),
        (
            json!({ "type": "error", "message": "Your input exceeds the context window of this model" }),
            json!({ "type": "provider-error", "message": "Your input exceeds the context window of this model", "classification": "context-overflow" }),
        ),
        (
            json!({ "type": "error" }),
            json!({ "type": "provider-error", "message": "OpenAI Responses stream error" }),
        ),
        (
            json!({ "type": "response.failed", "response": { "id": "r" } }),
            json!({ "type": "provider-error", "message": "OpenAI Responses response failed" }),
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(
            to_json(&events_for(std::slice::from_ref(&event), |_| {}).await),
            [expected],
            "{event}"
        );
    }
}

#[tokio::test]
async fn incomplete_finish_reasons_and_terminal_stops_stream() {
    for (reason, expected) in [
        ("max_output_tokens", "length"),
        ("content_filter", "content-filter"),
        ("other", "unknown"),
    ] {
        let events = events_for(
            &[
                json!({ "type": "response.output_text.delta", "item_id": "m", "delta": "x" }),
                json!({ "type": "response.incomplete", "response": { "incomplete_details": { "reason": reason } } }),
                // Anything after a terminal event is ignored.
                json!({ "type": "response.output_text.delta", "item_id": "m", "delta": "ignored" }),
            ],
            |_| {},
        )
        .await;
        assert_eq!(
            events
                .last()
                .map(|e| serde_json::to_value(e).unwrap()["reason"].clone()),
            Some(json!(expected))
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, LlmEvent::TextDelta { .. }))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn malformed_and_orphan_events_fail() {
    let mut request = base_request(Replay::new([sse(&[json!({ "type": 5 })])]));
    request.id = None;
    let error = generate(request).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid openai/openai-responses stream event"),
        "{error}"
    );
    let error = generate(base_request(Replay::new([sse(&[json!({ "type": "response.function_call_arguments.delta", "item_id": "nope", "delta": "{" })])])))
        .await
        .unwrap_err();
    assert_eq!(
        error.reason.message(),
        "OpenAI Responses tool argument delta is missing its tool call"
    );
}

#[tokio::test]
async fn http_errors_fail_before_stream_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"type":"invalid_request_error","message":"Bad request"}}"#,
        ))
        .mount(&server)
        .await;
    let mut route = responses_route(sse_transport(HttpExecutor::new().unwrap()));
    route.endpoint.base_url = Some(format!("{}/v1", server.uri()));
    route.auth = bearer_value("test");
    let mut request = request(model(route, "gpt-4.1-mini", "openai"));
    request.messages = vec![Message::user("hi")];
    let error = generate(request).await.unwrap_err();
    assert_eq!(error.reason.tag(), "InvalidRequest");
    assert!(error.to_string().contains("HTTP 400"));
    let received = &server.received_requests().await.unwrap()[0];
    assert_eq!(received.url.path(), "/v1/responses");
    assert_eq!(
        received.headers.get("authorization").unwrap(),
        "Bearer test"
    );
}

/// (request path, authorization header, received text messages).
type Handshake = (String, Option<String>, Vec<String>);

#[tokio::test]
async fn streams_over_websocket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Handshake>> = Arc::default();
    let record = seen.clone();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let hdr = record.clone();
        #[allow(clippy::result_large_err)]
        let callback = move |req: &Request, resp: Response| {
            let mut s = hdr.lock().unwrap();
            s.0 = req.uri().to_string();
            s.1 = req
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_owned());
            Ok(resp)
        };
        let mut ws = tokio_tungstenite::accept_hdr_async(tcp, callback)
            .await
            .unwrap();
        if let Some(Ok(WsMessage::Text(text))) = ws.next().await {
            record.lock().unwrap().2.push(text.as_str().to_owned());
        }
        for event in [
            json!({ "type": "response.output_text.delta", "item_id": "msg_1", "delta": "Hi" }),
            json!({ "type": "response.completed", "response": { "id": "resp_ws" } }),
        ] {
            ws.send(WsMessage::text(event.to_string())).await.unwrap();
        }
        // Keep the socket open: the terminal event must end the stream on its own.
        let _ = ws.next().await;
    });
    let mut route = responses_websocket_route();
    route.endpoint.base_url = Some(format!("http://{addr}/v1/"));
    route.auth = bearer_value("test");
    let mut request = request(model(route, "gpt-4.1-mini", "openai"));
    request.messages = vec![Message::user("Say hello.")];
    let response = generate(request).await.unwrap();
    assert_eq!(response.text(), "Hi");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.0, "/v1/responses");
    assert_eq!(seen.1.as_deref(), Some("Bearer test"));
    assert_eq!(seen.2.len(), 1);
    assert_eq!(
        seen.2[0],
        r#"{"type":"response.create","model":"gpt-4.1-mini","input":[{"role":"user","content":[{"type":"input_text","text":"Say hello."}]}],"store":false}"#
    );
    // The response stream was dropped after the terminal event, so the server sees the close.
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("socket closed after terminal event")
        .unwrap();
}

#[test]
fn websocket_route_metadata() {
    let route = responses_websocket_route();
    assert_eq!(route.id, "openai-responses-websocket");
    assert_eq!(route.transport.id(), "websocket-json");
    assert_eq!(route.protocol.id(), "openai-responses");
    let message =
        opencode_provider_adapters::protocols::openai::responses_body::web_socket_message(json!([
            1
        ]))
        .unwrap_err();
    assert_eq!(
        message.reason.message(),
        "OpenAI Responses WebSocket body must be a JSON object"
    );
    let text =
        opencode_provider_adapters::protocols::openai::responses_body::web_socket_message(json!({
            "model": "m", "input": [], "stream": true, "metadata": { "a": 1 }, "store": false,
        }))
        .unwrap();
    assert_eq!(
        text,
        r#"{"type":"response.create","model":"m","input":[],"store":false,"metadata":{"a":1}}"#
    );
}
