//! Purpose: LLM event publisher (publish-llm-event.ts port), stream half: text/reasoning event order
//! and joined end text, lazy single step start, step-finish settlement and token accounting,
//! provider error and failAssistant, protocol violations, and the projected transcript.
//! Scope: fixture-labelled recording sink delegating to the real p02 SqlEventStore over migrated SQLite.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/event.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/tests/support/publish.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations follow packages/core/src/session/runner/publish-llm-event.ts and
//! packages/core/test/session-runner-tool-events.test.ts.
//! Contract tests: session-runtime/tests/runner_publish.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/publish.rs"]
mod support;

use opencode_llm_api::event::{LlmEvent, Usage};
use opencode_llm_api::ids::FinishReason;
use opencode_session_runtime::runner::publish::PublishError;
use serde_json::json;
use support::{Fixture, call, media_result, text_events};

// fixture: recording sink over the real store.
#[tokio::test]
async fn text_stream_starts_step_once_and_ends_with_joined_text() {
    let mut f = Fixture::new().await;
    for event in text_events() {
        f.send(event).await;
    }
    assert_eq!(
        f.types(),
        [
            "session.next.step.started.1",
            "session.next.text.started.1",
            "session.next.text.delta",
            "session.next.text.delta",
            "session.next.text.ended.1",
        ]
    );
    let started = f.find("session.next.step.started.1").unwrap();
    assert_eq!(started["agent"], "build");
    assert_eq!(started["snapshot"], "snap_1");
    assert_eq!(
        started["model"],
        json!({"id": "model", "providerID": "provider"})
    );
    assert_eq!(
        f.find("session.next.text.ended.1").unwrap()["text"],
        "Hello"
    );
    assert!(f.publisher.has_assistant_started());
    assert!(f.publisher.has_active_assistant());
    let messages = f.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "assistant");
    assert_eq!(messages[0]["content"][0]["text"], "Hello");
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn step_finish_records_settlement_and_flushes_without_step_ended() {
    let mut f = Fixture::new().await;
    f.send(LlmEvent::ReasoningStart {
        id: "r1".into(),
        provider_metadata: None,
    })
    .await;
    f.send(LlmEvent::ReasoningDelta {
        id: "r1".into(),
        text: "think".into(),
        provider_metadata: None,
    })
    .await;
    f.send(LlmEvent::StepFinish {
        index: 0,
        reason: FinishReason::ToolCalls,
        usage: Some(Usage {
            non_cached_input_tokens: Some(10.0),
            output_tokens: Some(7.0),
            reasoning_tokens: Some(3.0),
            cache_read_input_tokens: Some(f64::NAN),
            cache_write_input_tokens: Some(-4.0),
            ..Usage::default()
        }),
        provider_metadata: None,
    })
    .await;
    assert!(
        !f.types()
            .iter()
            .any(|t| t.starts_with("session.next.step.ended"))
    );
    assert_eq!(
        f.find("session.next.reasoning.ended.1").unwrap()["text"],
        "think"
    );
    assert!(!f.publisher.has_active_assistant());
    let settlement = f.publisher.step_settlement().unwrap();
    assert_eq!(settlement.finish, "tool-calls");
    assert_eq!(
        serde_json::to_value(&settlement.tokens).unwrap(),
        json!({"input": 10, "output": 4, "reasoning": 3, "cache": {"read": 0, "write": 0}})
    );
    let err = f
        .publisher
        .publish(
            &LlmEvent::StepFinish {
                index: 1,
                reason: FinishReason::Stop,
                usage: None,
                provider_metadata: None,
            },
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Duplicate step finish");
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn provider_error_fails_assistant_once_and_flushes_open_text() {
    let mut f = Fixture::new().await;
    for event in text_events().into_iter().take(3) {
        f.send(event).await;
    }
    f.send(LlmEvent::ProviderError {
        message: "boom".into(),
        classification: None,
        retryable: None,
        provider_metadata: None,
    })
    .await;
    f.publisher.fail_assistant("again").await.unwrap();
    assert!(f.publisher.has_provider_error());
    assert!(!f.publisher.has_active_assistant());
    let types = f.types();
    assert_eq!(
        &types[types.len() - 2..],
        ["session.next.text.ended.1", "session.next.step.failed.2"]
    );
    assert_eq!(
        types.iter().filter(|t| t.contains("step.failed")).count(),
        1
    );
    assert_eq!(
        f.find("session.next.step.failed.2").unwrap()["error"],
        json!({"type": "unknown", "message": "boom"})
    );
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn fail_assistant_without_output_starts_step_first() {
    let mut f = Fixture::new().await;
    f.publisher.fail_assistant("interrupted").await.unwrap();
    assert_eq!(
        f.types(),
        ["session.next.step.started.1", "session.next.step.failed.2"]
    );
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn protocol_violations_match_baseline_messages() {
    let mut f = Fixture::new().await;
    let cases: Vec<(LlmEvent, &str)> = vec![
        (
            LlmEvent::TextDelta {
                id: "x".into(),
                text: "a".into(),
                provider_metadata: None,
            },
            "text delta before start: x",
        ),
        (
            LlmEvent::TextEnd {
                id: "x".into(),
                provider_metadata: None,
            },
            "text end before start: x",
        ),
        (
            LlmEvent::ToolInputDelta {
                id: "c".into(),
                name: "n".into(),
                text: "a".into(),
            },
            "Tool input delta before start: c",
        ),
        (media_result("nope", None), "Tool result before call: nope"),
        (
            LlmEvent::ToolError {
                id: "nope".into(),
                name: "read".into(),
                message: "m".into(),
                error: None,
                provider_metadata: None,
            },
            "Tool error before call: nope",
        ),
    ];
    for (event, message) in cases {
        let err = f.publisher.publish(&event, &[]).await.unwrap_err();
        assert!(matches!(err, PublishError::Protocol(_)));
        assert_eq!(err.to_string(), message);
    }
    f.send(call("dup", None)).await;
    let err = f
        .publisher
        .publish(&call("dup", None), &[])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Duplicate tool call: dup");
    let renamed = LlmEvent::ToolError {
        id: "dup".into(),
        name: "other".into(),
        message: "m".into(),
        error: None,
        provider_metadata: None,
    };
    let err = f.publisher.publish(&renamed, &[]).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Tool error name changed for dup: read -> other"
    );
    assert_eq!(
        f.publisher
            .assistant_message_id("zzz")
            .unwrap_err()
            .to_string(),
        "Unknown tool call: zzz"
    );
}
