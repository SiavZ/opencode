//! Purpose: fixture tests for the session schema modules, comparing Rust encoding with JSON
//! captured from the TypeScript baseline (bun 1.3.14, packages/schema at 907b3bc51).
//! Scope: integration tests through the public `opencode_schema` API.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/session_event.rs
//! `-- schema/src/session_message.rs
//! External:
//! |-- serde::Serialize
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::from_str
//! `-- serde_json::to_string
//! Used by:
//! `-- none, entrypoint
//! Invariants: every expected string is verbatim bun output (fixture: captured from bun at baseline).
//! Contract tests: this file.

use opencode_schema::event::Event;
use opencode_schema::session_event::{DurableEvent, SessionEvent, ToolFailed};
use opencode_schema::session_message::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;

fn rt<T: Serialize + DeserializeOwned>(input: &str) -> String {
    serde_json::to_string(&serde_json::from_str::<T>(input).unwrap()).unwrap()
}

#[test]
fn shell_message_keeps_base_time_position() {
    // fixture: captured from bun at baseline.
    let input = r#"{"output":"o","command":"c","callID":"x","type":"shell","time":{"completed":2,"created":1},"metadata":{"k":1},"id":"msg_1"}"#;
    assert_eq!(
        rt::<Message>(input),
        r#"{"id":"msg_1","metadata":{"k":1},"time":{"created":1,"completed":2},"type":"shell","callID":"x","command":"c","output":"o"}"#
    );
}

#[test]
fn assistant_message_with_all_parts() {
    // fixture: captured from bun at baseline.
    let input = r#"{"time":{"completed":2,"created":1},"error":{"message":"e","type":"unknown"},"tokens":{"cache":{"write":1,"read":2},"reasoning":0,"output":1,"input":2},"cost":0.1,"finish":"stop","snapshot":{"files":["a"],"end":"e","start":"s"},"content":[{"time":{"pruned":4,"completed":3,"ran":2,"created":1},"state":{"error":{"message":"x","type":"unknown"},"structured":{},"content":[{"text":"t","type":"text"}],"input":{},"status":"error","result":null},"provider":{"resultMetadata":{},"metadata":{"a":{"b":1}},"executed":false},"name":"n","id":"i","type":"tool"},{"time":{"created":1},"providerMetadata":{},"text":"r","id":"i","type":"reasoning"},{"text":"t","id":"i","type":"text"}],"model":{"providerID":"p","id":"m"},"agent":"a","type":"assistant","id":"msg_1"}"#;
    assert_eq!(
        rt::<Message>(input),
        r#"{"id":"msg_1","time":{"created":1,"completed":2},"type":"assistant","agent":"a","model":{"id":"m","providerID":"p"},"content":[{"type":"tool","id":"i","name":"n","provider":{"executed":false,"metadata":{"a":{"b":1}},"resultMetadata":{}},"state":{"status":"error","input":{},"content":[{"type":"text","text":"t"}],"structured":{},"error":{"type":"unknown","message":"x"},"result":null},"time":{"created":1,"ran":2,"completed":3,"pruned":4}},{"type":"reasoning","id":"i","text":"r","providerMetadata":{},"time":{"created":1}},{"type":"text","id":"i","text":"t"}],"snapshot":{"start":"s","end":"e","files":["a"]},"finish":"stop","cost":0.1,"tokens":{"input":2,"output":1,"reasoning":0,"cache":{"read":2,"write":1}},"error":{"type":"unknown","message":"e"}}"#
    );
}

#[test]
fn agent_switched_message() {
    // fixture: captured from bun at baseline.
    assert_eq!(
        rt::<Message>(r#"{"agent":"a","type":"agent-switched","time":{"created":5},"id":"msg_1"}"#),
        r#"{"id":"msg_1","time":{"created":5},"type":"agent-switched","agent":"a"}"#
    );
}

#[test]
fn durable_union_round_trips_and_dispatches() {
    // fixture: captured from bun at baseline.
    let input = r#"{"type":"session.next.step.ended","id":"evt_1","data":{"files":["a"],"tokens":{"cache":{"write":1,"read":2},"reasoning":0,"output":1,"input":2},"cost":0.5,"finish":"stop","assistantMessageID":"msg_1","sessionID":"ses_1","timestamp":10}}"#;
    let event: DurableEvent = serde_json::from_str(input).unwrap();
    assert!(matches!(event, DurableEvent::StepEnded(_)));
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"id":"evt_1","type":"session.next.step.ended","data":{"timestamp":10,"sessionID":"ses_1","assistantMessageID":"msg_1","finish":"stop","cost":0.5,"tokens":{"input":2,"output":1,"reasoning":0,"cache":{"read":2,"write":1}},"files":["a"]}}"#
    );
}

#[test]
fn event_envelope_optional_parts_order() {
    // Envelope order from event.ts define(): id, metadata, type, durable, location, data.
    let input = r#"{"data":{"provider":{"executed":true},"result":null,"error":{"message":"m","type":"unknown"},"callID":"c","assistantMessageID":"msg_1","sessionID":"ses_1","timestamp":1},"location":{"directory":"/d"},"durable":{"version":1,"seq":3,"aggregateID":"ses_1"},"type":"session.next.tool.failed","metadata":{},"id":"evt_1"}"#;
    assert_eq!(
        rt::<Event<ToolFailed>>(input),
        r#"{"id":"evt_1","metadata":{},"type":"session.next.tool.failed","durable":{"aggregateID":"ses_1","seq":3,"version":1},"location":{"directory":"/d"},"data":{"timestamp":1,"sessionID":"ses_1","assistantMessageID":"msg_1","callID":"c","error":{"type":"unknown","message":"m"},"result":null,"provider":{"executed":true}}}"#
    );
}

#[test]
fn invalid_session_values_fail() {
    let bad_message_id = r#"{"type":"system","id":"msg1","time":{"created":5},"text":"x"}"#;
    assert!(serde_json::from_str::<Message>(bad_message_id).is_err());
    let bad_time = r#"{"type":"system","id":"msg_1","time":{"created":"now"},"text":"x"}"#;
    assert!(serde_json::from_str::<Message>(bad_time).is_err());
    let unknown_event = r#"{"type":"session.next.nope","id":"evt_1","data":{}}"#;
    assert!(serde_json::from_str::<SessionEvent>(unknown_event).is_err());
    let bad_event_id = r#"{"type":"session.next.revert.cleared","id":"ev_1","data":{"timestamp":1,"sessionID":"ses_1"}}"#;
    assert!(serde_json::from_str::<SessionEvent>(bad_event_id).is_err());
}
