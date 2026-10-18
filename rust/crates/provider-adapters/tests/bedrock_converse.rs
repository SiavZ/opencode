//! Purpose: contract tests for the Bedrock Converse protocol: body lowering parity with
//! packages/llm/test/provider/bedrock-converse.test.ts, binary AWS event-stream decoding through
//! the real http-json transport framing, SigV4 signatures checked against values produced by the
//! baseline signer (aws4fetch) for fixed credentials and time, and replay of the recorded
//! bedrock-converse cassettes with exact request-body equality.
//! Scope: fixture-based (local wiremock server), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/event_stream.rs
//! `-- provider-adapters/tests/fixtures/anthropic-support/cassette.rs
//! External:
//! `-- wiremock::{Mock, MockServer, ResponseTemplate}
//! Used by:
//! `-- none, entrypoint
//! Invariants: the SigV4 expectations were generated once with aws4fetch (the baseline signer)
//! for datetime 20260101T000000Z; they must not be regenerated from the Rust signer.
//! Contract tests: provider-adapters/tests/bedrock_converse.rs

#[path = "fixtures/anthropic-support/cassette.rs"]
mod cassette;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opencode_llm_api::{
    CacheHint, CacheHintType, CachePolicy, ContentPart, FinishReason, GenerationOptions, LlmClient,
    LlmEvent, LlmRequest, LlmResponse, MediaData, Message, MessageRole, Model,
    ProviderFailureClassification, RouteClient, SystemPart, ToolCallPart, ToolChoice, ToolContent,
    ToolDefinition, ToolResultPart, ToolResultValue, generate,
};
use opencode_provider_adapters::auth::{
    AuthInput, CredentialHeader, Headers, Secret, StaticCredential,
};
use opencode_provider_adapters::protocols::bedrock::{self, BedrockCredentials, SigV4Auth};
use opencode_provider_adapters::route::ProtocolRoute;
use opencode_provider_adapters::transport::event_stream::encode_frame;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "anthropic.claude-3-5-sonnet-20240620-v1:0";

fn bearer_route(base: &str) -> Arc<ProtocolRoute> {
    let mut route = bedrock::route(cassette::event_stream_transport(bedrock::ID), None);
    route.endpoint.base_url = Some(base.to_owned());
    route.auth = Arc::new(CredentialHeader::bearer(Arc::new(StaticCredential {
        source: "AWS_BEARER_TOKEN_BEDROCK".into(),
        value: Some(Secret::new("test-bearer")),
    })));
    Arc::new(route)
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

fn eph(ttl: Option<f64>) -> Option<CacheHint> {
    Some(CacheHint {
        kind: CacheHintType::Ephemeral,
        ttl_seconds: ttl,
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

fn tool_result(id: &str, name: &str, value: ToolResultValue, cache: Option<CacheHint>) -> Message {
    Message::tool(ToolResultPart {
        id: id.into(),
        name: name.into(),
        result: value,
        provider_executed: None,
        cache,
        metadata: None,
        provider_metadata: None,
    })
}

fn media(media_type: &str, data: MediaData, filename: Option<&str>) -> ContentPart {
    ContentPart::Media {
        media_type: media_type.into(),
        data,
        filename: filename.map(str::to_owned),
        metadata: None,
    }
}

fn text_cached(text: &str, cache: Option<CacheHint>) -> ContentPart {
    ContentPart::Text {
        text: text.into(),
        cache,
        metadata: None,
        provider_metadata: None,
    }
}

fn none_cache(mut r: LlmRequest) -> LlmRequest {
    r.cache = Some(CachePolicy::Named("none".into()));
    r
}

fn offline() -> Model {
    Model::new(
        MODEL,
        "amazon-bedrock",
        bearer_route("https://bedrock-runtime.test"),
    )
}

fn base_request() -> LlmRequest {
    let mut r = LlmRequest::new(offline());
    r.id = Some("req_1".into());
    r.system = vec![SystemPart::text("You are concise.")];
    r.messages = vec![Message::user("Say hello.")];
    r.generation = Some(GenerationOptions {
        max_tokens: Some(64.0),
        temperature: Some(0.0),
        ..Default::default()
    });
    none_cache(r)
}

fn prepare(r: LlmRequest) -> Value {
    RouteClient.prepare(r).expect("prepare").body
}

fn prepare_err(r: LlmRequest) -> String {
    RouteClient.prepare(r).expect_err("error").reason.message()
}

#[test]
fn prepares_converse_target_and_top_k() {
    assert_eq!(
        prepare(base_request()),
        json!({
            "modelId": MODEL,
            "messages": [{ "role": "user", "content": [{ "text": "Say hello." }] }],
            "system": [{ "text": "You are concise." }],
            "inferenceConfig": { "maxTokens": 64, "temperature": 0 }
        })
    );
    let mut r = base_request();
    r.generation.as_mut().unwrap().top_k = Some(40.0);
    let body = prepare(r);
    assert_eq!(
        body["inferenceConfig"],
        json!({ "maxTokens": 64, "temperature": 0 })
    );
    assert_eq!(body["additionalModelRequestFields"], json!({ "top_k": 40 }));
    assert_eq!(
        bedrock::path("us.amazon.nova-micro-v1:0"),
        "/model/us.amazon.nova-micro-v1%3A0/converse-stream"
    );
}

#[test]
fn lowers_system_updates_tools_and_tool_history() {
    let mut r = LlmRequest::new(offline());
    r.messages = vec![
        Message::user("Before."),
        Message::system("Update."),
        Message::assistant(vec![ContentPart::text("After.")]),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([
            { "role": "user", "content": [{ "text": "Before." }, { "text": "<system-update>\nUpdate.\n</system-update>" }] },
            { "role": "assistant", "content": [{ "text": "After." }] }
        ])
    );

    let mut r = base_request();
    r.tools = vec![tool(
        "lookup",
        "Lookup data",
        json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
    )];
    r.tool_choice = Some(ToolChoice::parse("required"));
    assert_eq!(
        prepare(r)["toolConfig"],
        json!({
            "tools": [{ "toolSpec": { "name": "lookup", "description": "Lookup data", "inputSchema": { "json": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } } } }],
            "toolChoice": { "any": {} }
        })
    );

    let mut r = LlmRequest::new(offline());
    r.messages = vec![
        Message::user("What is the weather?"),
        Message::assistant(vec![call(
            "tool_1",
            "lookup",
            json!({ "query": "weather" }),
        )]),
        tool_result(
            "tool_1",
            "lookup",
            ToolResultValue::Json(json!({ "forecast": "sunny" })),
            None,
        ),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([
            { "role": "user", "content": [{ "text": "What is the weather?" }] },
            { "role": "assistant", "content": [{ "toolUse": { "toolUseId": "tool_1", "name": "lookup", "input": { "query": "weather" } } }] },
            { "role": "user", "content": [{ "toolResult": { "toolUseId": "tool_1", "content": [{ "json": { "forecast": "sunny" } }], "status": "success" } }] }
        ])
    );

    let mut r = LlmRequest::new(offline());
    r.messages = vec![
        Message::user("Capture the screen."),
        Message::assistant(vec![call("tool_1", "screenshot", json!({}))]),
        tool_result(
            "tool_1",
            "screenshot",
            ToolResultValue::Content(vec![
                ToolContent::Text {
                    text: "Screenshot captured.".into(),
                },
                ToolContent::File {
                    uri: "data:image/png;base64,AAAA".into(),
                    mime: "image/png".into(),
                    name: None,
                },
            ]),
            None,
        ),
    ];
    assert_eq!(
        prepare(none_cache(r))["messages"][2]["content"][0]["toolResult"]["content"],
        json!([{ "text": "Screenshot captured." }, { "image": { "format": "png", "source": { "bytes": "AAAA" } } }])
    );

    let mut r = LlmRequest::new(offline());
    r.messages = vec![tool_result(
        "t",
        "x",
        ToolResultValue::Content(vec![ToolContent::File {
            uri: "data:application/pdf;base64,AAAA".into(),
            mime: "application/pdf".into(),
            name: None,
        }]),
        None,
    )];
    assert_eq!(
        prepare_err(r),
        "Bedrock Converse only supports image media in tool results"
    );
}

#[test]
fn lowers_media_images_documents_and_rejects_unsupported() {
    let mut r = LlmRequest::new(offline());
    r.messages = vec![Message::new(
        MessageRole::User,
        vec![
            ContentPart::text("What is in this image?"),
            media("image/png", MediaData::Text("AAAA".into()), None),
            media("image/jpeg", MediaData::Text("BBBB".into()), None),
            media("image/jpg", MediaData::Text("CCCC".into()), None),
            media("image/webp", MediaData::Text("DDDD".into()), None),
            media("image/png", MediaData::Bytes(vec![1, 2, 3, 4, 5]), None),
            media(
                "application/pdf",
                MediaData::Text("UERGREFUQQ==".into()),
                Some("report.pdf"),
            ),
            media("text/csv", MediaData::Text("Q1NWREFUQQ==".into()), None),
        ],
    )];
    assert_eq!(
        prepare(none_cache(r))["messages"][0]["content"],
        json!([
            { "text": "What is in this image?" },
            { "image": { "format": "png", "source": { "bytes": "AAAA" } } },
            { "image": { "format": "jpeg", "source": { "bytes": "BBBB" } } },
            { "image": { "format": "jpeg", "source": { "bytes": "CCCC" } } },
            { "image": { "format": "webp", "source": { "bytes": "DDDD" } } },
            { "image": { "format": "png", "source": { "bytes": "AQIDBAU=" } } },
            { "document": { "format": "pdf", "name": "report.pdf", "source": { "bytes": "UERGREFUQQ==" } } },
            { "document": { "format": "csv", "name": "document.csv", "source": { "bytes": "Q1NWREFUQQ==" } } }
        ])
    );
    for (mime, expected) in [
        (
            "image/svg+xml",
            "Bedrock Converse does not support image media type image/svg+xml",
        ),
        (
            "application/x-tar",
            "Bedrock Converse does not support media type application/x-tar",
        ),
    ] {
        let mut r = LlmRequest::new(offline());
        r.messages = vec![Message::new(
            MessageRole::User,
            vec![media(mime, MediaData::Text("x".into()), Some("a.tar"))],
        )];
        assert!(prepare_err(r).contains(expected));
    }
}

#[test]
fn cache_points_ttl_and_breakpoint_cap() {
    let mut r = LlmRequest::new(offline());
    r.system = vec![SystemPart {
        cache: eph(None),
        ..SystemPart::text("System prefix.")
    }];
    r.messages = vec![
        Message::new(
            MessageRole::User,
            vec![text_cached("User prefix.", eph(None))],
        ),
        Message::assistant(vec![text_cached("Assistant prefix.", eph(None))]),
    ];
    let body = prepare(r);
    assert_eq!(
        body["system"],
        json!([{ "text": "System prefix." }, { "cachePoint": { "type": "default" } }])
    );
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": [{ "text": "User prefix." }, { "cachePoint": { "type": "default" } }] },
            { "role": "assistant", "content": [{ "text": "Assistant prefix." }, { "cachePoint": { "type": "default" } }] }
        ])
    );

    let body = prepare(base_request());
    assert_eq!(body["system"], json!([{ "text": "You are concise." }]));

    let mut r = LlmRequest::new(offline());
    r.system = vec![SystemPart {
        cache: eph(Some(3600.0)),
        ..SystemPart::text("system")
    }];
    r.messages = vec![Message::user("hi")];
    assert_eq!(
        prepare(r)["system"],
        json!([{ "text": "system" }, { "cachePoint": { "type": "default", "ttl": "1h" } }])
    );

    let mut r = LlmRequest::new(offline());
    let mut lookup = tool(
        "lookup",
        "lookup",
        json!({ "type": "object", "properties": {} }),
    );
    lookup.cache = eph(None);
    r.tools = vec![lookup];
    r.messages = vec![
        Message::user("What's the weather?"),
        Message::assistant(vec![call("call_1", "lookup", json!({}))]),
        tool_result(
            "call_1",
            "lookup",
            ToolResultValue::Json(json!({ "temp": 72 })),
            eph(None),
        ),
    ];
    let body = prepare(none_cache(r));
    assert_eq!(
        body["toolConfig"]["tools"][1],
        json!({ "cachePoint": { "type": "default" } })
    );
    assert_eq!(
        body["messages"][2]["content"][1],
        json!({ "cachePoint": { "type": "default" } })
    );

    let mut r = LlmRequest::new(offline());
    r.system = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|t| SystemPart {
            cache: eph(None),
            ..SystemPart::text(*t)
        })
        .collect();
    r.messages = vec![Message::user("hi")];
    let body = prepare(r);
    assert_eq!(
        body["system"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p.get("cachePoint").is_some())
            .count(),
        4
    );
}

#[test]
fn lowers_reasoning_signatures() {
    let mut r = LlmRequest::new(offline());
    r.messages = vec![Message::assistant(vec![ContentPart::Reasoning {
        text: "Let me think.".into(),
        encrypted: None,
        metadata: None,
        provider_metadata: Some(
            [("bedrock".to_owned(), obj(json!({ "signature": "sig_1" })))]
                .into_iter()
                .collect(),
        ),
    }])];
    assert_eq!(
        prepare(none_cache(r))["messages"],
        json!([{ "role": "assistant", "content": [{ "reasoningContent": { "reasoningText": { "text": "Let me think.", "signature": "sig_1" } } }] }])
    );
}

fn event_stream(events: &[(&str, Value)]) -> Vec<u8> {
    events
        .iter()
        .flat_map(|(kind, payload)| {
            encode_frame(
                &[
                    (":message-type", "event"),
                    (":event-type", kind),
                    (":content-type", "application/json"),
                ],
                payload.to_string().as_bytes(),
            )
        })
        .collect()
}

async fn run_stream(
    events: &[(&str, Value)],
    request: impl FnOnce(Model) -> LlmRequest,
) -> Result<LlmResponse, opencode_llm_api::LlmError> {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(format!(
            "/model/{}/converse-stream",
            "anthropic.claude-3-5-sonnet-20240620-v1%3A0"
        )))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer test-bearer",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/vnd.amazon.eventstream")
                .set_body_bytes(event_stream(events)),
        )
        .mount(&server)
        .await;
    let model = Model::new(MODEL, "amazon-bedrock", bearer_route(&server.uri()));
    generate(&RouteClient, request(model)).await
}

fn with_model(model: Model) -> LlmRequest {
    let mut r = base_request();
    r.model = model;
    r
}

#[tokio::test]
async fn decodes_text_message_stop_and_metadata_usage() {
    let response = run_stream(
        &[
            ("messageStart", json!({ "role": "assistant" })),
            (
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "Hello" } }),
            ),
            (
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "!" } }),
            ),
            ("contentBlockStop", json!({ "contentBlockIndex": 0 })),
            ("messageStop", json!({ "stopReason": "end_turn" })),
            (
                "metadata",
                json!({ "usage": { "inputTokens": 5, "outputTokens": 2, "totalTokens": 7 } }),
            ),
        ],
        with_model,
    )
    .await
    .unwrap();
    assert_eq!(response.text(), "Hello!");
    let finishes: Vec<&LlmEvent> = response
        .events
        .iter()
        .filter(|e| matches!(e, LlmEvent::Finish { .. }))
        .collect();
    assert_eq!(finishes.len(), 1);
    assert_eq!(response.finish_reason, FinishReason::Stop);
    let usage = response.usage.unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (Some(5.0), Some(2.0), Some(7.0))
    );
}

#[tokio::test]
async fn assembles_streamed_tool_call_input() {
    let response = run_stream(
        &[
            ("messageStart", json!({ "role": "assistant" })),
            ("contentBlockStart", json!({ "contentBlockIndex": 0, "start": { "toolUse": { "toolUseId": "tool_1", "name": "lookup" } } })),
            ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "toolUse": { "input": "{\"query\"" } } })),
            ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "toolUse": { "input": ":\"weather\"}" } } })),
            ("contentBlockStop", json!({ "contentBlockIndex": 0 })),
            ("messageStop", json!({ "stopReason": "tool_use" })),
        ],
        |m| {
            let mut r = with_model(m);
            r.tools = vec![tool("lookup", "Lookup", json!({ "type": "object" }))];
            r
        },
    )
    .await
    .unwrap();
    let calls: Vec<&LlmEvent> = response.tool_calls().collect();
    assert_eq!(
        calls,
        vec![&LlmEvent::ToolCall {
            id: "tool_1".into(),
            name: "lookup".into(),
            input: json!({ "query": "weather" }),
            provider_executed: None,
            provider_metadata: None
        }]
    );
    let deltas: Vec<&LlmEvent> = response
        .events
        .iter()
        .filter(|e| matches!(e, LlmEvent::ToolInputDelta { .. }))
        .collect();
    assert_eq!(
        deltas,
        vec![
            &LlmEvent::ToolInputDelta {
                id: "tool_1".into(),
                name: "lookup".into(),
                text: "{\"query\"".into()
            },
            &LlmEvent::ToolInputDelta {
                id: "tool_1".into(),
                name: "lookup".into(),
                text: ":\"weather\"}".into()
            },
        ]
    );
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
}

#[tokio::test]
async fn decodes_reasoning_and_preserves_signatures() {
    let response = run_stream(
        &[
            ("messageStart", json!({ "role": "assistant" })),
            ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "text": "Let me think." } } })),
            ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "signature": "sig_1" } } })),
            ("contentBlockStop", json!({ "contentBlockIndex": 0 })),
            ("messageStop", json!({ "stopReason": "end_turn" })),
        ],
        with_model,
    )
    .await
    .unwrap();
    assert_eq!(response.reasoning(), "Let me think.");
    assert!(
        response.events.contains(&LlmEvent::ReasoningEnd {
            id: "reasoning-0".into(),
            provider_metadata: Some(
                [("bedrock".to_owned(), obj(json!({ "signature": "sig_1" })))]
                    .into_iter()
                    .collect()
            ),
        })
    );
}

#[tokio::test]
async fn stream_exceptions_become_provider_errors() {
    let response = run_stream(
        &[
            ("messageStart", json!({ "role": "assistant" })),
            ("throttlingException", json!({ "message": "Slow down" })),
        ],
        with_model,
    )
    .await
    .unwrap();
    assert!(response.events.contains(&LlmEvent::ProviderError {
        message: "Slow down".into(),
        classification: None,
        retryable: Some(true),
        provider_metadata: None
    }));

    let response = run_stream(
        &[(
            "validationException",
            json!({ "message": "Input is too long for requested model" }),
        )],
        with_model,
    )
    .await
    .unwrap();
    assert_eq!(
        response.events,
        vec![LlmEvent::ProviderError {
            message: "Input is too long for requested model".into(),
            classification: Some(ProviderFailureClassification::ContextOverflow),
            retryable: Some(false),
            provider_metadata: None
        }]
    );

    let response = run_stream(
        &[("internalServerException", json!({ "message": "boom" }))],
        with_model,
    )
    .await
    .unwrap();
    assert!(matches!(
        &response.events[0],
        LlmEvent::ProviderError {
            retryable: Some(true),
            ..
        }
    ));
}

#[tokio::test]
async fn rejects_requests_with_no_auth_path() {
    let server = MockServer::start().await;
    let mut route = bedrock::route(cassette::event_stream_transport(bedrock::ID), None);
    route.endpoint.base_url = Some(server.uri());
    let mut r = base_request();
    r.model = Model::new(MODEL, "amazon-bedrock", Arc::new(route));
    let error = generate(&RouteClient, r).await.expect_err("no auth");
    assert!(
        error
            .reason
            .message()
            .contains("Bedrock Converse requires either route bearer auth or AWS credentials")
    );
}

fn fixed_credentials(session_token: Option<&str>) -> BedrockCredentials {
    BedrockCredentials {
        region: "us-east-1".into(),
        access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
        secret_access_key: Secret::new("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
        session_token: session_token.map(Secret::new),
    }
}

/// 2026-01-01T00:00:00Z.
fn fixed_time() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_767_225_600)
}

#[tokio::test]
async fn sigv4_matches_baseline_signer_signatures() {
    let body = r#"{"modelId":"us.amazon.nova-micro-v1:0","messages":[{"role":"user","content":[{"text":"Say hello."}]}]}"#;
    let url = "https://bedrock-runtime.us-east-1.amazonaws.com/model/us.amazon.nova-micro-v1%3A0/converse-stream";
    let request = base_request();
    // Expected values produced by aws4fetch `AwsV4Signer` (service bedrock, datetime 20260101T000000Z).
    let cases = [
        (
            None,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/bedrock/aws4_request, SignedHeaders=anthropic-beta;host;x-amz-date, Signature=394a25f8ad1cf1b15d6eedadf36af288fdca115737febb00c02a34143ca8afec",
        ),
        (
            Some("FwoGZXIvYXdzEXAMPLETOKEN"),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/bedrock/aws4_request, SignedHeaders=anthropic-beta;host;x-amz-date;x-amz-security-token, Signature=63bc4153fb18751cc0bbfa5b8c75c9ad72a816bdd60c42b930c765588078aea4",
        ),
    ];
    for (token, authorization) in cases {
        let auth = SigV4Auth {
            credentials: Some(fixed_credentials(token)),
            time: Some(fixed_time()),
        };
        let input = AuthInput {
            request: &request,
            method: "POST",
            url,
            body,
        };
        let headers = opencode_provider_adapters::auth::Auth::apply(
            &auth,
            input,
            Headers::from([("anthropic-beta".to_owned(), "x".to_owned())]),
        )
        .await
        .unwrap();
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some(authorization)
        );
        assert_eq!(
            headers.get("x-amz-date").map(String::as_str),
            Some("20260101T000000Z")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("x-amz-security-token").map(String::as_str),
            token
        );
        assert!(!format!("{auth:?}").contains("wJalrXUtnFEMI"));
    }
}

// ---------------------------------------------------------------------------
// Recorded cassettes (binary AWS event-stream bodies decoded by the transport framing).
// ---------------------------------------------------------------------------

fn recorded_model(replay: &cassette::Replay) -> Model {
    let mut route = bedrock::route(
        cassette::event_stream_transport(bedrock::ID),
        Some(BedrockCredentials {
            region: "us-east-1".into(),
            access_key_id: "fixture".into(),
            secret_access_key: Secret::new("fixture"),
            session_token: None,
        }),
    );
    assert_eq!(
        route.endpoint.base_url.as_deref(),
        Some("https://bedrock-runtime.us-east-1.amazonaws.com")
    );
    route.endpoint.base_url = Some(replay.base_url(""));
    Model::new(
        "us.amazon.nova-micro-v1:0",
        "amazon-bedrock",
        Arc::new(route),
    )
}

fn weather_tool() -> ToolDefinition {
    tool(
        "get_weather",
        "Get current weather for a city.",
        json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"], "additionalProperties": false }),
    )
}

fn usage_triple(response: &LlmResponse) -> (Option<f64>, Option<f64>, Option<f64>) {
    let u = response.usage.clone().unwrap();
    (u.input_tokens, u.output_tokens, u.total_tokens)
}

fn recorded_request(model: Model, system: &str, prompt: &str, max: f64) -> LlmRequest {
    let mut r = LlmRequest::new(model);
    r.system = vec![SystemPart::text(system)];
    r.messages = vec![Message::user(prompt)];
    r.generation = Some(GenerationOptions {
        max_tokens: Some(max),
        temperature: Some(0.0),
        ..Default::default()
    });
    none_cache(r)
}

#[tokio::test]
async fn recorded_streams_text() {
    let replay = cassette::Replay::start("bedrock-converse/streams-text.json").await;
    let r = recorded_request(
        recorded_model(&replay),
        "Reply with the single word 'Hello'.",
        "Say hello.",
        16.0,
    );
    let response = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    let seen = replay.seen();
    let auth = seen[0]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=fixture/")
            && auth.contains("/us-east-1/bedrock/aws4_request")
    );
    assert_eq!(response.text(), "Hello");
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(usage_triple(&response), (Some(12.0), Some(2.0), Some(14.0)));
}

#[tokio::test]
async fn recorded_streams_a_tool_call() {
    let replay = cassette::Replay::start("bedrock-converse/streams-a-tool-call.json").await;
    let mut r = recorded_request(
        recorded_model(&replay),
        "Call tools exactly as requested.",
        "Call get_weather with city exactly Paris.",
        80.0,
    );
    r.tools = vec![weather_tool()];
    r.tool_choice = Some(ToolChoice::named("get_weather"));
    let response = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    let calls: Vec<(String, Value)> = response
        .tool_calls()
        .filter_map(|e| match e {
            LlmEvent::ToolCall { name, input, .. } => Some((name.clone(), input.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        vec![("get_weather".to_owned(), json!({ "city": "Paris" }))]
    );
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(
        usage_triple(&response),
        (Some(419.0), Some(16.0), Some(435.0))
    );
}

#[tokio::test]
async fn recorded_drives_a_tool_loop() {
    let replay = cassette::Replay::start("bedrock-converse/drives-a-tool-loop.json").await;
    let mut r = recorded_request(
        recorded_model(&replay),
        "Use the get_weather tool, then answer in one short sentence.",
        "What is the weather in Paris?",
        80.0,
    );
    r.tools = vec![weather_tool()];
    let first = generate(&RouteClient, r.clone()).await.unwrap();
    assert_eq!(first.finish_reason, FinishReason::ToolCalls);
    let (id, name, input) = first
        .tool_calls()
        .find_map(|e| match e {
            LlmEvent::ToolCall {
                id, name, input, ..
            } => Some((id.clone(), name.clone(), input.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(input, json!({ "city": "Paris" }));
    r.messages.push(Message::assistant(vec![
        ContentPart::text(first.text()),
        call(&id, &name, input),
    ]));
    r.messages.push(tool_result(
        &id,
        &name,
        ToolResultValue::Json(json!({ "temperature": 22, "condition": "sunny" })),
        None,
    ));
    let second = generate(&RouteClient, r).await.unwrap();
    replay.assert_requests();
    assert_eq!(second.finish_reason, FinishReason::Stop);
    assert!(second.text().contains("Paris"));
    let step_finishes: Vec<FinishReason> = first
        .events
        .iter()
        .chain(&second.events)
        .filter_map(|e| match e {
            LlmEvent::StepFinish { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert_eq!(step_finishes, [FinishReason::ToolCalls, FinishReason::Stop]);
}
