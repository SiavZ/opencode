//! Purpose: LLM event publisher (publish-llm-event.ts port), tool half: tool calls published before
//! settlement, local vs provider-executed results (compatibility `result` kept only for hosted
//! tools, no local settlement required), error results, failUnsettledTools hosted-only filtering,
//! and streamed tool input joining.
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
//! Contract tests: session-runtime/tests/runner_publish_tools.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/publish.rs"]
mod support;

use opencode_llm_api::event::LlmEvent;
use opencode_llm_api::message::ToolResultValue;
use opencode_session_runtime::runner::publish::requires_local_settlement;
use serde_json::{Value, json};
use support::{Fixture, call, media_result};

// fixture: recording sink over the real store.
#[tokio::test]
async fn tool_call_without_input_events_synthesizes_input_and_is_published_before_settlement() {
    let mut f = Fixture::new().await;
    let event = call("call-1", None);
    assert!(requires_local_settlement(&event));
    f.send(event).await;
    let assistant = f.publisher.assistant_message_id("call-1").unwrap();
    f.publisher
        .publish(&media_result("call-1", None), &["out/a.txt".into()])
        .await
        .unwrap();
    assert_eq!(
        f.types(),
        [
            "session.next.step.started.1",
            "session.next.tool.input.started.1",
            "session.next.tool.input.ended.1",
            "session.next.tool.called.1",
            "session.next.tool.success.1",
        ]
    );
    let called = f.find("session.next.tool.called.1").unwrap();
    assert_eq!(called["assistantMessageID"], assistant.as_str());
    assert_eq!(called["input"], json!({"path": "pixel.png"}));
    assert_eq!(called["provider"], json!({"executed": false}));
    let success = f.find("session.next.tool.success.1").unwrap();
    // Local success keeps no compatibility `result` and serializes the media payload once.
    assert!(success.get("result").is_none());
    assert_eq!(success.to_string().matches("iVBORw0KGgo").count(), 1);
    assert_eq!(success["outputPaths"], json!(["out/a.txt"]));
    assert_eq!(
        success["structured"],
        json!({"type": "media", "mime": "image/png"})
    );
    let messages = f.messages().await;
    let tool = &messages[0]["content"][0];
    assert_eq!(tool["state"]["status"], "completed");
    assert_eq!(tool["state"]["input"], json!({"path": "pixel.png"}));
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn provider_executed_result_is_published_with_result_and_needs_no_local_settlement() {
    let mut f = Fixture::new().await;
    let event = call("call-h", Some(true));
    assert!(!requires_local_settlement(&event));
    f.send(event).await;
    f.send(media_result("call-h", Some(true))).await;
    let success = f.find("session.next.tool.success.1").unwrap();
    assert_eq!(success["provider"], json!({"executed": true}));
    assert_eq!(success["result"]["type"], "content");
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn error_result_fails_tool_without_success() {
    let mut f = Fixture::new().await;
    f.send(call("call-b", None)).await;
    f.send(LlmEvent::ToolResult {
        id: "call-b".into(),
        name: "read".into(),
        result: ToolResultValue::Error(json!("Cannot read binary file")),
        output: None,
        provider_executed: None,
        provider_metadata: None,
    })
    .await;
    assert!(f.find("session.next.tool.success.1").is_none());
    let failed = f.find("session.next.tool.failed.1").unwrap();
    assert_eq!(
        failed["error"],
        json!({"type": "unknown", "message": "Cannot read binary file"})
    );
    assert_eq!(
        failed["result"],
        json!({"type": "error", "value": "Cannot read binary file"})
    );
    // A late error result after settlement is ignored, a late success is a protocol violation.
    f.send(LlmEvent::ToolResult {
        id: "call-b".into(),
        name: "read".into(),
        result: ToolResultValue::Error(json!("again")),
        output: None,
        provider_executed: None,
        provider_metadata: None,
    })
    .await;
    let err = f
        .publisher
        .publish(&media_result("call-b", None), &[])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Duplicate tool result: call-b");
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn fail_unsettled_tools_respects_hosted_only_and_settled_calls() {
    let mut f = Fixture::new().await;
    f.send(call("local", None)).await;
    f.send(LlmEvent::ToolCall {
        id: "hosted".into(),
        name: "web".into(),
        input: json!("q"),
        provider_executed: Some(true),
        provider_metadata: Some(serde_json::from_value(json!({"p": {"k": 1}})).unwrap()),
    })
    .await;
    f.send(call("done", None)).await;
    f.send(media_result("done", None)).await;

    f.publisher
        .fail_unsettled_tools("Provider did not return a tool result", true)
        .await
        .unwrap();
    let failed: Vec<Value> = f
        .recorder
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|(n, _)| n == "session.next.tool.failed.1")
        .map(|(_, d)| d.clone())
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["callID"], "hosted");
    assert_eq!(
        failed[0]["provider"],
        json!({"executed": true, "metadata": {"p": {"k": 1}}})
    );
    assert_eq!(
        f.find("session.next.tool.called.1").map(|_| ()),
        Some(()),
        "tool calls were published"
    );

    f.publisher
        .fail_unsettled_tools("Tool execution interrupted", false)
        .await
        .unwrap();
    let failed_ids: Vec<String> = f
        .recorder
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|(n, _)| n == "session.next.tool.failed.1")
        .map(|(_, d)| d["callID"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(failed_ids, ["hosted", "local"]);
    // Non-object input is wrapped as `{ value }`.
    let hosted_called = f
        .recorder
        .published
        .lock()
        .unwrap()
        .iter()
        .find(|(n, d)| n == "session.next.tool.called.1" && d["callID"] == "hosted")
        .map(|(_, d)| d.clone())
        .unwrap();
    assert_eq!(hosted_called["input"], json!({"value": "q"}));
}

// fixture: recording sink over the real store.
#[tokio::test]
async fn streamed_tool_input_is_joined_and_flushed() {
    let mut f = Fixture::new().await;
    f.send(LlmEvent::ToolInputStart {
        id: "c".into(),
        name: "bash".into(),
        provider_metadata: None,
    })
    .await;
    for chunk in ["{\"cmd\":", "\"ls\"}"] {
        f.send(LlmEvent::ToolInputDelta {
            id: "c".into(),
            name: "bash".into(),
            text: chunk.into(),
        })
        .await;
    }
    f.publisher.flush().await.unwrap();
    assert_eq!(
        f.find("session.next.tool.input.ended.1").unwrap()["text"],
        "{\"cmd\":\"ls\"}"
    );
    // Input already ended by flush: the call must not end it again.
    f.send(LlmEvent::ToolCall {
        id: "c".into(),
        name: "bash".into(),
        input: json!({"cmd": "ls"}),
        provider_executed: None,
        provider_metadata: None,
    })
    .await;
    assert_eq!(
        f.types()
            .iter()
            .filter(|t| *t == "session.next.tool.input.ended.1")
            .count(),
        1
    );
}
