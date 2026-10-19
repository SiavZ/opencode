//! Purpose: contract tests for session-api semantics over schema-owned values: exact-retry
//! equivalence, delivery spelling and `session_message` row split/rebuild.
//! Scope: fixture-based (hand-written baseline-shaped JSON), no I/O.
//! Uses:
//! rust/crates/
//! |-- schema/src/values.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/lib.rs
//! `-- session-api/src/message.rs
//! External:
//! |-- admission::delivery_str
//! |-- message::from_row
//! |-- message::to_row
//! |-- pretty_assertions::assert_eq
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: row data keeps encoded key order without `id`/`type`.
//! Contract tests: session-api/tests/wire.rs

use opencode_schema::values::{DateTimeUtc, NonNegativeInt};
use opencode_session_api::admission::{self, equivalent};
use opencode_session_api::message::{self, AssistantContent, ToolState};
use opencode_session_api::{Admitted, Delivery, Message, MessageID, Prompt, SessionID};
use pretty_assertions::assert_eq;
use serde_json::json;

fn admitted(delivery: Delivery) -> Admitted {
    Admitted {
        admitted_seq: NonNegativeInt::new(1).unwrap(),
        id: MessageID::new("msg_1").unwrap(),
        session_id: SessionID::new("ses_1").unwrap(),
        prompt: Prompt::text("hi"),
        delivery,
        time_created: DateTimeUtc::from_millis(0).unwrap(),
        promoted_seq: None,
    }
}

#[test]
fn exact_retry_requires_session_prompt_and_delivery() {
    let stored = admitted(Delivery::Queue);
    let session = SessionID::new("ses_1").unwrap();
    assert!(equivalent(
        &stored,
        &session,
        &Prompt::text("hi"),
        Delivery::Queue
    ));
    assert!(!equivalent(
        &stored,
        &session,
        &Prompt::text("hi"),
        Delivery::Steer
    ));
    assert!(!equivalent(
        &stored,
        &session,
        &Prompt::text("other"),
        Delivery::Queue
    ));
    let other = SessionID::new("ses_2").unwrap();
    assert!(!equivalent(
        &stored,
        &other,
        &Prompt::text("hi"),
        Delivery::Queue
    ));
}

#[test]
fn delivery_spelling_round_trips() {
    for delivery in [Delivery::Steer, Delivery::Queue] {
        let text = admission::delivery_str(delivery);
        assert_eq!(admission::parse_delivery(text), Some(delivery));
        assert_eq!(serde_json::to_value(delivery).unwrap(), json!(text));
    }
    assert_eq!(admission::parse_delivery("later"), None);
}

#[test]
fn message_row_round_trip_matches_projector_split() {
    // fixture: shape of an encoded baseline assistant message without id/type.
    let data = json!({
        "time": {"created": 5},
        "agent": "build",
        "model": {"id": "m", "providerID": "p"},
        "content": [
            {"type": "text", "id": "t1", "text": "hello"},
            {"type": "tool", "id": "c1", "name": "read",
             "state": {"status": "completed", "input": {"path": "a"}, "content": [{"type": "text", "text": "x"}], "structured": {}},
             "time": {"created": 6, "completed": 7}}
        ]
    });
    let rebuilt =
        message::from_row("msg_a", "assistant", data.as_object().unwrap().clone()).unwrap();
    let Message::Assistant(assistant) = &rebuilt else {
        panic!("expected assistant");
    };
    assert!(matches!(
        &assistant.content[1],
        AssistantContent::Tool(tool) if matches!(tool.state, ToolState::Completed(_))
    ));
    let row = message::to_row(&rebuilt).unwrap();
    assert_eq!((row.id.as_str(), row.kind.as_str()), ("msg_a", "assistant"));
    assert_eq!(serde_json::Value::Object(row.data), data);
    assert_eq!(message::created_millis(&rebuilt), 5);
}
