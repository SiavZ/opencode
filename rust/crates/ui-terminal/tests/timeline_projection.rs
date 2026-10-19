//! Purpose: verify durable V2 and legacy message projection through the public decoder.
//! Scope: controlled wire fixtures, no session/runtime mocks.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: lifecycle failures and unsupported input remain observable.
//! Contract tests: this file
use opencode_ui_terminal::timeline::{Message, Part, ProjectionError, tool::Status};
use serde_json::json;

#[test]
fn projects_actual_v2_content_and_all_tool_states() {
    for (status, expected) in [
        ("pending", Status::Pending),
        ("running", Status::Running),
        ("completed", Status::Completed),
        ("error", Status::Error),
    ] {
        let message = Message::from_value(&json!({"type":"assistant","content":[
            {"type":"text","text":"hello"},
            {"type":"reasoning","text":"thinking"},
            {"type":"tool","name":"read","state":{"status":status,"input":{"filePath":"a.rs"},"structured":{"title":"Read a.rs"},"content":[{"type":"text","text":"one"},{"type":"text","text":"two"}],"error":{"type":"unknown","message":"denied"}}}
        ],"error":{"message":"provider failed"}})).unwrap();
        assert_eq!(message.parts[0], Part::Text("hello".into()));
        assert_eq!(message.parts[1], Part::Reasoning("thinking".into()));
        let Part::Tool(tool) = &message.parts[2] else {
            panic!("tool omitted")
        };
        assert_eq!(tool.status, expected);
        assert_eq!(tool.output, "one\ntwo");
        assert_eq!(tool.error.as_deref(), Some("denied"));
        assert_eq!(tool.title.as_deref(), Some("Read a.rs"));
        assert_eq!(
            message.parts[3],
            Part::Text("Error: provider failed".into())
        );
    }
}
#[test]
fn legacy_metadata_and_output_are_retained() {
    let message = Message::from_value(&json!({"info":{"role":"assistant"},"parts":[{"type":"tool","tool":"edit","state":{"status":"completed","input":{"filePath":"a.rs"},"metadata":{"diff":"@@ -1 +1 @@\n-a\n+b"},"output":"changed"}}]})).unwrap();
    let Part::Tool(tool) = &message.parts[0] else {
        panic!("tool omitted")
    };
    assert_eq!(tool.name, "edit");
    assert_eq!(tool.output, "changed");
    assert!(tool.structured["diff"].as_str().unwrap().contains("-a"));
}
#[test]
fn malformed_and_unknown_content_fail_instead_of_fake_pending() {
    assert_eq!(
        Message::from_value(&json!({})),
        Err(ProjectionError::Invalid("role/type"))
    );
    assert_eq!(
        Message::from_value(&json!({"type":"assistant","content":{}})),
        Err(ProjectionError::Invalid("content/parts"))
    );
    assert!(matches!(
        Message::from_value(
            &json!({"type":"assistant","content":[{"type":"tool","name":"x","state":{"status":"invented"}}]})
        ),
        Err(ProjectionError::Unsupported(_))
    ));
    assert!(matches!(
        Message::from_value(&json!({"type":"assistant","content":[{"type":"unported"}]})),
        Err(ProjectionError::Unsupported(_))
    ));
}
#[test]
fn projects_user_attachments_shell_switch_and_compaction() {
    let user = Message::from_value(
        &json!({"type":"user","text":"look","files":[{"uri":"file:///fixture/image.png"}]}),
    )
    .unwrap();
    assert_eq!(user.parts.len(), 2);
    assert_eq!(user.parts[0], Part::Text("look".into()));
    let shell = Message::from_value(
        &json!({"type":"shell","command":"echo hello","output":"hello","time":{"completed":1}}),
    )
    .unwrap();
    let Part::Tool(tool) = &shell.parts[0] else {
        panic!("shell omitted")
    };
    assert_eq!(tool.status, Status::Completed);
    assert_eq!(tool.output, "hello");
    assert_eq!(
        Message::from_value(&json!({"type":"agent-switched","agent":"build"}))
            .unwrap()
            .parts,
        [Part::Text("Agent: build".into())]
    );
    assert_eq!(
        Message::from_value(&json!({"type":"compaction","summary":"earlier work"}))
            .unwrap()
            .parts,
        [Part::Text("Compacted: earlier work".into())]
    );
}
