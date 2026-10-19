//! Purpose: contract tests for the llm-api wire shapes, option precedence, response
//! assembly and `generate` terminal handling. Fixture-based (deterministic in-memory route).
//! Scope: integration tests through the public crate API.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/lib.rs
//! `-- llm-api/src/options.rs
//! External:
//! |-- futures::executor::block_on
//! |-- futures::stream
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- serde_json::to_string
//! |-- serde_json::to_value
//! `-- stream::iter
//! Used by:
//! `-- none, entrypoint
//! Invariants: expected JSON literals are transcribed from packages/llm/src/schema shapes.
//! Contract tests: llm-api/tests/contract.rs

use std::sync::Arc;

use futures::executor::block_on;
use futures::stream;
use opencode_llm_api::error::LlmErrorReason;
use opencode_llm_api::event::{subtract_tokens, total_tokens};
use opencode_llm_api::options::merge_json_records;
use opencode_llm_api::*;
use serde_json::{Value, json};

#[derive(Debug)]
struct FixtureRoute {
    defaults: RouteDefaults,
    events: Vec<Result<LlmEvent, LlmError>>,
}

impl Route for FixtureRoute {
    fn id(&self) -> &str {
        "fixture"
    }
    fn protocol(&self) -> &str {
        "fixture"
    }
    fn transport(&self) -> &str {
        "memory"
    }
    fn provider(&self) -> Option<&ProviderId> {
        None
    }
    fn defaults(&self) -> &RouteDefaults {
        &self.defaults
    }
    fn prepare(&self, request: &LlmRequest) -> Result<PreparedRequest, LlmError> {
        Ok(PreparedRequest {
            id: request.id.clone().unwrap_or_else(|| "request".into()),
            route: "fixture".into(),
            protocol: "fixture".into(),
            model: request.model.id.clone(),
            provider: request.model.provider.clone(),
            body: serde_json::to_value(&request.generation).unwrap(),
            metadata: None,
        })
    }
    fn stream(&self, _request: LlmRequest) -> LlmEventStream {
        Box::pin(stream::iter(self.events.clone()))
    }
}

fn model(defaults: RouteDefaults, events: Vec<Result<LlmEvent, LlmError>>) -> Model {
    Model::new("m", "p", Arc::new(FixtureRoute { defaults, events }))
}

#[test]
fn event_wire_shapes_match_baseline() {
    let event = LlmEvent::ToolCall {
        id: "c1".into(),
        name: "read".into(),
        input: json!({"path": "a"}),
        provider_executed: Some(true),
        provider_metadata: None,
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"tool-call","id":"c1","name":"read","input":{"path":"a"},"providerExecuted":true}"#
    );
    let finish = LlmEvent::Finish {
        reason: FinishReason::ToolCalls,
        usage: Some(Usage {
            input_tokens: Some(10.0),
            output_tokens: Some(2.5),
            ..Default::default()
        }),
        provider_metadata: None,
    };
    assert_eq!(
        serde_json::to_string(&finish).unwrap(),
        r#"{"type":"finish","reason":"tool-calls","usage":{"inputTokens":10,"outputTokens":2.5}}"#
    );
    let round: LlmEvent = serde_json::from_str(r#"{"type":"step-start","index":0}"#).unwrap();
    assert_eq!(round, LlmEvent::StepStart { index: 0 });
}

#[test]
fn content_parts_round_trip() {
    let msg = Message::tool(ToolResultPart {
        id: "c1".into(),
        name: "read".into(),
        result: ToolResultValue::Text(json!("ok")),
        provider_executed: None,
        cache: Some(CacheHint {
            kind: CacheHintType::Ephemeral,
            ttl_seconds: None,
        }),
        metadata: None,
        provider_metadata: None,
    });
    let text = serde_json::to_string(&msg).unwrap();
    assert_eq!(
        text,
        r#"{"role":"tool","content":[{"type":"tool-result","id":"c1","name":"read","result":{"type":"text","value":"ok"},"cache":{"type":"ephemeral"}}]}"#
    );
    assert_eq!(serde_json::from_str::<Message>(&text).unwrap(), msg);
}

#[test]
fn errors_preserve_tags_and_retryability() {
    let rate = LlmError::new(
        "RequestExecutor",
        "execute",
        LlmErrorReason::RateLimit {
            message: "slow down".into(),
            retry_after_ms: Some(1500.0),
            rate_limit: None,
            provider_metadata: None,
            http: None,
        },
    );
    assert!(rate.retryable());
    assert_eq!(rate.retry_after_ms(), Some(1500.0));
    assert_eq!(rate.to_string(), "RequestExecutor.execute: slow down");
    let value = serde_json::to_value(&rate).unwrap();
    assert_eq!(value["reason"]["_tag"], "RateLimit");
    assert_eq!(value["reason"]["retryAfterMs"], json!(1500));

    let no_route = LlmErrorReason::NoRoute {
        route: "r".into(),
        provider: "p".into(),
        model: "m".into(),
    };
    assert_eq!(no_route.message(), "No LLM route for p/m using r");
    assert!(!no_route.retryable());
    assert!(!LlmError::cancelled("x", "y").retryable());
    let overflow = LlmErrorReason::InvalidRequest {
        message: "too long".into(),
        parameter: None,
        classification: Some(ProviderFailureClassification::ContextOverflow),
        provider_metadata: None,
        http: None,
    };
    assert!(overflow.is_context_overflow());
    assert_eq!(
        serde_json::to_value(&overflow).unwrap()["classification"],
        "context-overflow"
    );
}

#[test]
fn json_records_deep_merge_later_wins() {
    let a = json!({"a": 1, "nested": {"x": 1, "y": 1}})
        .as_object()
        .cloned()
        .unwrap();
    let b = json!({"b": 2, "nested": {"y": 2}})
        .as_object()
        .cloned()
        .unwrap();
    let merged = merge_json_records([Some(&a), None, Some(&b)]).unwrap();
    assert_eq!(
        Value::Object(merged),
        json!({"a": 1, "nested": {"x": 1, "y": 2}, "b": 2})
    );
    assert_eq!(merge_json_records([None, None]), None);
}

#[test]
fn request_option_precedence_route_model_request() {
    let defaults = RouteDefaults {
        generation: Some(GenerationOptions {
            max_tokens: Some(100.0),
            temperature: Some(0.1),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut m = model(defaults, vec![]);
    m.defaults = Some(ModelDefaults {
        generation: Some(GenerationOptions {
            temperature: Some(0.5),
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut request = LlmRequest::new(m);
    request.generation = Some(GenerationOptions {
        top_p: Some(0.9),
        ..Default::default()
    });
    let prepared = RouteClient.prepare(request).unwrap();
    assert_eq!(
        prepared.body,
        json!({"maxTokens": 100, "temperature": 0.5, "topP": 0.9})
    );
}

#[test]
fn route_header_shorthand_merges_before_model_and_request_headers() {
    let mut selected = model(
        RouteDefaults {
            headers: Some(
                [
                    ("route".into(), "kept".into()),
                    ("shared".into(), "shorthand".into()),
                ]
                .into(),
            ),
            http: Some(HttpOptions {
                headers: Some(
                    [
                        ("http".into(), "kept".into()),
                        ("shared".into(), "http".into()),
                    ]
                    .into(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    );
    selected.defaults = Some(ModelDefaults {
        http: Some(HttpOptions {
            headers: Some([("shared".into(), "model".into())].into()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut request = LlmRequest::new(selected);
    request.http = Some(HttpOptions {
        headers: Some([("shared".into(), "request".into())].into()),
        ..Default::default()
    });
    let resolved = resolve_request_options(request);
    let headers = resolved.http.unwrap().headers.unwrap();
    assert_eq!(headers["http"], "kept");
    assert_eq!(headers["shared"], "request");
    assert_eq!(headers["route"], "kept");
    assert_eq!(headers.len(), 3);
}

#[test]
fn reducer_assembles_interleaved_blocks_in_first_seen_order() {
    let events = vec![
        LlmEvent::ReasoningDelta {
            id: "r".into(),
            text: "think".into(),
            provider_metadata: None,
        },
        LlmEvent::TextDelta {
            id: "t".into(),
            text: "Hel".into(),
            provider_metadata: None,
        },
        LlmEvent::TextDelta {
            id: "t".into(),
            text: "lo".into(),
            provider_metadata: None,
        },
        LlmEvent::ToolInputDelta {
            id: "c".into(),
            name: "x".into(),
            text: "{}".into(),
        },
        LlmEvent::ToolCall {
            id: "c".into(),
            name: "x".into(),
            input: json!({}),
            provider_executed: None,
            provider_metadata: None,
        },
        LlmEvent::ProviderError {
            message: "boom".into(),
            classification: None,
            retryable: None,
            provider_metadata: None,
        },
    ];
    let response = LlmResponse::from_events(events).unwrap();
    assert_eq!(response.finish_reason, FinishReason::Error);
    assert_eq!(response.text(), "Hello");
    assert_eq!(response.reasoning(), "think");
    assert_eq!(response.message.content.len(), 3);
    assert_eq!(response.message.content[0], ContentPart::reasoning("think"));
    assert_eq!(response.message.content[1], ContentPart::text("Hello"));
    assert!(matches!(
        response.message.content[2],
        ContentPart::ToolCall(_)
    ));
    assert_eq!(response.tool_calls().count(), 1);
}

#[test]
fn generate_requires_terminal_event() {
    let ok = model(
        RouteDefaults::default(),
        vec![
            Ok(LlmEvent::TextDelta {
                id: "t".into(),
                text: "hi".into(),
                provider_metadata: None,
            }),
            Ok(LlmEvent::Finish {
                reason: FinishReason::Stop,
                usage: None,
                provider_metadata: None,
            }),
        ],
    );
    let response = block_on(generate(&RouteClient, LlmRequest::new(ok))).unwrap();
    assert_eq!(response.text(), "hi");

    let truncated = model(
        RouteDefaults::default(),
        vec![Ok(LlmEvent::TextDelta {
            id: "t".into(),
            text: "hi".into(),
            provider_metadata: None,
        })],
    );
    let err = block_on(generate(&RouteClient, LlmRequest::new(truncated))).unwrap_err();
    assert_eq!(
        err.to_string(),
        "ProviderShared.stream: Provider stream ended without a terminal finish event"
    );

    let failing = model(
        RouteDefaults::default(),
        vec![Err(LlmError::cancelled("Route", "stream"))],
    );
    assert!(
        block_on(generate(&RouteClient, LlmRequest::new(failing)))
            .unwrap_err()
            .is_cancelled()
    );
}

#[test]
fn usage_helpers_follow_shared_policy() {
    assert_eq!(total_tokens(Some(3.0), None, None), Some(3.0));
    assert_eq!(total_tokens(None, None, None), None);
    assert_eq!(total_tokens(Some(1.0), Some(1.0), Some(9.0)), Some(9.0));
    assert_eq!(subtract_tokens(Some(5.0), Some(7.0)), Some(0.0));
    assert_eq!(subtract_tokens(None, Some(1.0)), None);
    let usage = Usage {
        output_tokens: Some(4.0),
        reasoning_tokens: Some(6.0),
        ..Default::default()
    };
    assert_eq!(usage.visible_output_tokens(), 0.0);
}

#[test]
fn tool_choice_parse() {
    assert_eq!(ToolChoice::parse("auto").kind, ToolChoiceType::Auto);
    assert_eq!(ToolChoice::parse("bash"), ToolChoice::named("bash"));
    assert_eq!(
        serde_json::to_string(&ToolChoice::named("bash")).unwrap(),
        r#"{"type":"tool","name":"bash"}"#
    );
}
