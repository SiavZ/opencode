//! Purpose: verify agent, command, and prompt-input codecs against real baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! `-- schema/src/agent.rs
//! External:
//! |-- serde::Serialize
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Value
//! |-- serde_json::Value::Null
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture strings are byte-identical to Bun decode/encode output.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_agent

use opencode_schema::agent::{
    Color, CommandInfo, Info, PromptInputFileAttachment, PromptInputPrompt,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-15, /tmp/p01home/agent.ts.
const AGENT_INFO: &str = r#"{"id":"test","request":{"headers":{},"body":{}},"mode":"all","hidden":false,"permissions":[]}"#;
const COMMAND_INFO: &str = r#"{"name":"test","template":"echo {{input}}"}"#;
const PROMPT_INPUT: &str = r#"{"text":"hello"}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Info>(AGENT_INFO);
    roundtrip::<CommandInfo>(COMMAND_INFO);
    roundtrip::<PromptInputPrompt>(PROMPT_INPUT);
    roundtrip::<PromptInputFileAttachment>(r#"{"uri":"file:///test.txt"}"#);
    roundtrip::<Color>(r##""#a0B9fF""##);
    roundtrip::<Color>(r#""accent""#);
}

#[test]
fn order_is_canonical_and_unknown_fields_are_removed() {
    let decoded: CommandInfo = serde_json::from_value(json!({
        "template": "t", "name": "n", "unknown": 1
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"name":"n","template":"t"}"#
    );
    assert!(decoded.description.is_none());
    assert!(decoded.agent.is_none());
    assert!(decoded.model.is_none());
    assert!(decoded.subtask.is_none());
}

#[test]
fn optional_null_and_wrong_shapes_are_rejected() {
    for key in ["description", "agent", "model", "subtask"] {
        let mut input: serde_json::Value = serde_json::from_str(COMMAND_INFO).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<CommandInfo>(input).is_err(),
            "{key}"
        );
    }
    for key in ["name", "template"] {
        let mut input: serde_json::Value = serde_json::from_str(COMMAND_INFO).unwrap();
        input.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<CommandInfo>(input).is_err(),
            "{key}"
        );
    }
}

#[test]
fn prompt_input_omits_absent_and_rejects_null() {
    for key in ["files", "agents"] {
        let mut input: serde_json::Value = serde_json::from_str(PROMPT_INPUT).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<PromptInputPrompt>(input).is_err(),
            "{key}"
        );
    }
    let decoded: PromptInputPrompt = serde_json::from_value(json!({
        "text": "hi", "files": [{"uri": "u", "source": {"start": 1, "end": 2, "text": "t"}}]
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"text":"hi","files":[{"uri":"u","source":{"start":1,"end":2,"text":"t"}}]}"#
    );
}
