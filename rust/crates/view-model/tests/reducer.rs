//! Purpose: port of `packages/app/src/context/server-session-v2-reducer.test.ts:9-156` plus
//! idempotence, compaction, shell and wire-shape cases. Fixture-based (no live producer).
//! Scope: integration tests through the public `V2SessionReducer` boundary.
//! Uses:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! `-- view-model/src/source.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: events are built as baseline-shaped JSON and decoded through serde.
//! Contract tests: self

use opencode_view_model::source::{AssistantContent, CompactionState, ToolState};
use opencode_view_model::{SessionEvent, SessionMessage, V2SessionReducer};
use serde_json::{Value, json};

fn event(id: &str, kind: &str, data: Value) -> SessionEvent {
    serde_json::from_value(json!({
        "id": id,
        "created": 1,
        "type": kind,
        "location": { "directory": "/repo" },
        "durable": { "aggregateID": "ses_1", "seq": 1, "version": 1 },
        "data": data,
    }))
    .expect("event decodes")
}

struct Harness {
    reducer: V2SessionReducer,
    messages: Vec<SessionMessage>,
}

impl Harness {
    fn new() -> Self {
        Self {
            reducer: V2SessionReducer::new(),
            messages: vec![],
        }
    }
    fn apply(
        &mut self,
        id: &str,
        kind: &str,
        data: Value,
    ) -> Option<opencode_view_model::Reduction> {
        let result = self.reducer.reduce(&self.messages, &event(id, kind, data));
        if let Some(r) = &result {
            self.messages = r.messages.clone();
        }
        result
    }
    fn step(&mut self) {
        self.apply(
            "evt_step",
            "session.step.started",
            json!({ "sessionID": "ses_1", "assistantMessageID": "msg_assistant", "agent": "build",
                    "model": { "id": "model", "providerID": "provider" } }),
        );
    }
}

#[test]
fn projects_promoted_input_and_streaming_assistant_content() {
    let mut h = Harness::new();
    h.apply(
        "evt_admitted",
        "session.input.admitted",
        json!({ "sessionID": "ses_1", "inputID": "msg_user",
                "input": { "type": "user", "delivery": "steer", "data": { "text": "hello" } } }),
    );
    assert!(
        h.messages.is_empty(),
        "admission alone must not create a visible message"
    );
    h.apply(
        "evt_promoted",
        "session.input.promoted",
        json!({ "sessionID": "ses_1", "inputID": "msg_user" }),
    );
    h.step();
    let base = json!({ "sessionID": "ses_1", "assistantMessageID": "msg_assistant", "ordinal": 0 });
    let with = |extra: Value| {
        let mut v = base.clone();
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    h.apply("evt_text_start", "session.text.started", base.clone());
    h.apply(
        "evt_text_delta",
        "session.text.delta",
        with(json!({ "delta": "hel" })),
    );
    assert_eq!(
        h.messages[1].as_assistant().unwrap().content,
        vec![AssistantContent::Text { text: "hel".into() }]
    );
    h.apply(
        "evt_text_end",
        "session.text.ended",
        with(json!({ "text": "hello" })),
    );

    assert!(
        matches!(&h.messages[0], SessionMessage::User { id, text, .. } if id == "msg_user" && text == "hello")
    );
    let a = h.messages[1].as_assistant().unwrap();
    assert_eq!(a.id, "msg_assistant");
    assert_eq!(
        a.content,
        vec![AssistantContent::Text {
            text: "hello".into()
        }]
    );
}

#[test]
fn folds_tool_retry_and_completion_events() {
    let mut h = Harness::new();
    h.step();
    let tool = |extra: Value| {
        let mut v = json!({ "sessionID": "ses_1", "assistantMessageID": "msg_assistant", "callID": "call_1" });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    h.apply(
        "evt_tool_start",
        "session.tool.input.started",
        tool(json!({ "name": "bash" })),
    );
    h.apply(
        "evt_tool_delta",
        "session.tool.input.delta",
        tool(json!({ "delta": "{}" })),
    );
    h.apply(
        "evt_tool_called",
        "session.tool.called",
        tool(json!({ "input": {}, "executed": true })),
    );
    h.apply(
        "evt_tool_success",
        "session.tool.success",
        tool(json!({ "metadata": {}, "content": [{ "type": "text", "text": "done" }], "executed": true })),
    );
    h.apply(
        "evt_retry",
        "session.retry.scheduled",
        json!({ "sessionID": "ses_1", "assistantMessageID": "msg_assistant", "attempt": 2, "at": 10,
                "error": { "type": "ProviderError", "message": "retry" } }),
    );
    assert!(h.messages[0].as_assistant().unwrap().retry.is_some());
    h.apply(
        "evt_done",
        "session.execution.succeeded",
        json!({ "sessionID": "ses_1" }),
    );

    let a = h.messages[0].as_assistant().unwrap();
    assert!(a.retry.is_none());
    let AssistantContent::Tool(t) = &a.content[0] else {
        panic!("tool expected")
    };
    assert_eq!(t.id, "call_1");
    let ToolState::Completed { content, .. } = &t.state else {
        panic!("completed expected")
    };
    assert_eq!(
        serde_json::to_value(content).unwrap(),
        json!([{ "type": "text", "text": "done" }])
    );
}

#[test]
fn requests_hydration_when_promotion_admission_was_missed() {
    let mut reducer = V2SessionReducer::new();
    let r = reducer
        .reduce(
            &[],
            &event(
                "evt_promoted",
                "session.input.promoted",
                json!({ "sessionID": "ses_1", "inputID": "msg_user" }),
            ),
        )
        .unwrap();
    assert_eq!(r.session_id, "ses_1");
    assert_eq!(r.missing.as_deref(), Some("msg_user"));
    assert!(r.touched.is_empty());
}

#[test]
fn ignores_events_without_session_and_unknown_types() {
    let mut reducer = V2SessionReducer::new();
    assert!(
        reducer
            .reduce(&[], &event("evt_x", "session.text.delta", json!({})))
            .is_none()
    );
    assert!(
        reducer
            .reduce(
                &[],
                &event("evt_x", "server.connected", json!({ "sessionID": "ses_1" }))
            )
            .is_none()
    );
}

#[test]
fn appends_are_idempotent_and_clear_drops_pending() {
    let mut h = Harness::new();
    h.apply(
        "evt_a",
        "session.agent.selected",
        json!({ "sessionID": "ses_1", "agent": "plan" }),
    );
    h.apply(
        "evt_a",
        "session.agent.selected",
        json!({ "sessionID": "ses_1", "agent": "plan" }),
    );
    assert_eq!(h.messages.len(), 1);
    assert_eq!(h.messages[0].id(), "msg_a");
    h.apply(
        "evt_adm",
        "session.input.admitted",
        json!({ "sessionID": "ses_1", "inputID": "msg_u", "input": { "type": "synthetic", "delivery": "queue", "data": { "text": "x" } } }),
    );
    assert_eq!(h.reducer.pending_len(), 1);
    h.reducer.clear("ses_1");
    let r = h
        .apply(
            "evt_p",
            "session.input.promoted",
            json!({ "sessionID": "ses_1", "inputID": "msg_u" }),
        )
        .unwrap();
    assert_eq!(r.missing.as_deref(), Some("msg_u"));
}

#[test]
fn second_step_completes_previous_open_assistant() {
    let mut h = Harness::new();
    h.step();
    let r = h
        .apply(
            "evt_step2",
            "session.step.started",
            json!({ "sessionID": "ses_1", "assistantMessageID": "msg_b", "agent": "build",
                    "model": { "id": "m", "providerID": "p" } }),
        )
        .unwrap();
    assert_eq!(
        r.touched,
        vec!["msg_assistant".to_owned(), "msg_b".to_owned()]
    );
    assert_eq!(
        h.messages[0].as_assistant().unwrap().time.completed,
        Some(1)
    );
    assert_eq!(h.messages[1].as_assistant().unwrap().time.completed, None);
}

#[test]
fn compaction_lifecycle_and_model_previous() {
    let mut h = Harness::new();
    h.step();
    h.apply(
        "evt_m",
        "session.model.selected",
        json!({ "sessionID": "ses_1", "model": { "id": "x", "providerID": "y" } }),
    );
    let SessionMessage::ModelSwitched { previous, .. } = &h.messages[1] else {
        panic!()
    };
    assert_eq!(previous.as_ref().unwrap().id, "model");

    h.apply(
        "evt_c",
        "session.compaction.started",
        json!({ "sessionID": "ses_1", "reason": "auto", "recent": "r" }),
    );
    h.apply(
        "evt_cd",
        "session.compaction.delta",
        json!({ "sessionID": "ses_1", "text": "sum" }),
    );
    h.apply(
        "evt_ce",
        "session.compaction.ended",
        json!({ "sessionID": "ses_1", "reason": "auto", "text": "summary", "recent": "r" }),
    );
    let SessionMessage::Compaction { id, state, .. } = &h.messages[2] else {
        panic!()
    };
    assert_eq!(id, "msg_c");
    assert_eq!(
        state,
        &CompactionState::Completed {
            summary: "summary".into(),
            recent: "r".into()
        }
    );
    assert_eq!(
        serde_json::to_value(&h.messages[2]).unwrap(),
        json!({ "type": "compaction", "id": "msg_c", "time": { "created": 1 }, "reason": "auto",
                "status": "completed", "summary": "summary", "recent": "r" })
    );
}

#[test]
fn shell_started_and_ended() {
    let mut h = Harness::new();
    let shell = json!({ "id": "sh_1", "status": "running", "command": "ls", "cwd": "/", "shell": "zsh",
                        "file": "f", "metadata": {}, "time": { "started": 1 } });
    h.apply(
        "evt_s",
        "session.shell.started",
        json!({ "sessionID": "ses_1", "shell": shell }),
    );
    let mut ended = shell.clone();
    ended["status"] = json!("exited");
    ended["exit"] = json!(0);
    h.apply(
        "evt_se",
        "session.shell.ended",
        json!({ "sessionID": "ses_1", "shell": ended,
                "output": { "output": "a\n", "cursor": 2, "size": 2, "truncated": false } }),
    );
    let SessionMessage::Shell {
        status,
        exit,
        output,
        time,
        ..
    } = &h.messages[0]
    else {
        panic!()
    };
    assert_eq!(status, "exited");
    assert_eq!(exit, &Some(json!(0)));
    assert_eq!(output.as_ref().unwrap().output, "a\n");
    assert_eq!(time.completed, Some(1));
}

#[test]
fn tool_failure_from_streaming_resets_input() {
    let mut h = Harness::new();
    h.step();
    let tool = json!({ "sessionID": "ses_1", "assistantMessageID": "msg_assistant", "callID": "c", "name": "read" });
    h.apply("evt_t", "session.tool.input.started", tool.clone());
    let mut failed = tool.clone();
    failed["error"] = json!({ "type": "unknown", "message": "boom" });
    failed["executed"] = json!(false);
    h.apply("evt_tf", "session.tool.failed", failed);
    let AssistantContent::Tool(t) = &h.messages[0].as_assistant().unwrap().content[0] else {
        panic!()
    };
    assert_eq!(
        serde_json::to_value(&t.state).unwrap(),
        json!({ "status": "error", "input": {}, "error": { "type": "unknown", "message": "boom" }, "metadata": {} })
    );
}
