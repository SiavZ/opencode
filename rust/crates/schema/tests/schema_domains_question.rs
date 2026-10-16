//! Purpose: verify question metadata and events against baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/question.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are Bun output, JS trim and optional null rejection are preserved.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_question

use opencode_schema::event::Event;
use opencode_schema::question::{
    self, Answer, Info, Prompt, QuestionOption, Rejected, Replied, Reply, Request, Tool,
};
use serde::{Serialize, de::DeserializeOwned};

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline
const OPTION: &str = r#"{"label":"Yes","description":"Affirmative"}"#;
const INFO: &str = r#"{"question":"Do you agree?","header":"Agree?","options":[{"label":"Yes","description":"Affirmative"}],"multiple":true,"custom":false}"#;
const INFO_NO_OPT: &str = r#"{"question":"Q?","header":"H","options":[]}"#;
const PROMPT: &str = r#"{"question":"Do you agree?","header":"Agree?","options":[{"label":"Yes","description":"Affirmative"}],"multiple":true}"#;
const TOOL: &str = r#"{"messageID":"msg_123","callID":"call_123"}"#;
const REQUEST: &str = r#"{"id":"que_00000000000000000000000000","sessionID":"ses_00000000000000000000000000","questions":[{"question":"Do you agree?","header":"Agree?","options":[{"label":"Yes","description":"Affirmative"}],"multiple":true,"custom":false}],"tool":{"messageID":"msg_123","callID":"call_123"}}"#;
const REPLY: &str = r#"{"answers":[["Yes"]]}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<QuestionOption>(OPTION);
    roundtrip::<Info>(INFO);
    roundtrip::<Info>(INFO_NO_OPT);
    roundtrip::<Prompt>(PROMPT);
    roundtrip::<Tool>(TOOL);
    roundtrip::<Request>(REQUEST);
    roundtrip::<Reply>(REPLY);
    roundtrip::<Event<Request>>(&format!(
        r#"{{"id":"evt_probe","type":"question.v2.asked","data":{REQUEST}}}"#
    ));
    roundtrip::<Event<Replied>>(
        r#"{"id":"evt_probe","type":"question.v2.replied","data":{"sessionID":"ses_1","requestID":"que_1","answers":[["Yes"]]}}"#,
    );
    roundtrip::<Event<Rejected>>(
        r#"{"id":"evt_probe","type":"question.v2.rejected","data":{"sessionID":"ses_1","requestID":"que_1"}}"#,
    );
}

#[test]
fn optional_fields_omit_missing_and_reject_null() {
    for key in ["multiple", "custom"] {
        let mut input: serde_json::Value = serde_json::from_str(INFO_NO_OPT).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
    let mut input: serde_json::Value = serde_json::from_str(REQUEST).unwrap();
    input["tool"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<Request>(input).is_err());

    let mut input: serde_json::Value = serde_json::from_str(REQUEST).unwrap();
    input.as_object_mut().unwrap().remove("tool");
    let decoded: Request = serde_json::from_value(input).unwrap();
    assert!(decoded.tool.is_none());
}

#[test]
fn inventory_and_unknown_fields_match_baseline() {
    assert_eq!(
        question::DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        [
            "question.v2.asked",
            "question.v2.replied",
            "question.v2.rejected"
        ]
    );
    assert_eq!(question::DEFINITIONS.len(), 3);
    assert!(question::DEFINITIONS.iter().all(|d| d.durable.is_none()));

    let decoded: Info =
        serde_json::from_str(r#"{"question":"Q","header":"H","options":[],"unknown":"value"}"#)
            .unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"question":"Q","header":"H","options":[]}"#
    );
}

#[test]
fn validates_required_fields() {
    assert!(serde_json::from_str::<Info>(r#"{"question":"Q","header":"H"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"id":"que_1","sessionID":"ses_1"}"#).is_err());
    assert!(serde_json::from_str::<Replied>(r#"{"sessionID":"ses_1"}"#).is_err());
    assert!(serde_json::from_str::<Rejected>(r#"{"requestID":"que_1"}"#).is_err());
}

#[test]
fn answer_type_is_vec_string() {
    let answer: Answer = serde_json::from_str(r#"["Yes","No"]"#).unwrap();
    assert_eq!(answer, vec!["Yes".to_string(), "No".to_string()]);
    assert_eq!(serde_json::to_string(&answer).unwrap(), r#"["Yes","No"]"#);
}
