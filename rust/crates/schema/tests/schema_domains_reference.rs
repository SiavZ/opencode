//! Purpose: verify reference and skill codecs against real baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/reference.rs
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
//! Contract tests: cargo test -p opencode-schema --test schema_domains_reference

use opencode_schema::event::Event;
use opencode_schema::reference::{
    EmbeddedSource, GitSource, Info, SkillInfo, SkillSource, Source, Updated,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-15.
const REF_INFO: &str = r#"{"name":"my-ref","path":"/abs/path","description":"a ref","hidden":false,"source":{"type":"local","path":"/abs/path"}}"#;
const GIT_SOURCE: &str = r#"{"type":"git","repository":"https://github.com/a/b","branch":"main","description":"git ref","hidden":true}"#;
const SKILL_INFO: &str = r#"{"name":"my-skill","description":"a skill","slash":true,"location":"/abs/skill","content":"content"}"#;
const EMBEDDED_SOURCE: &str = r#"{"type":"embedded","skill":{"name":"my-skill","description":"a skill","slash":true,"location":"/abs/skill","content":"content"}}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Info>(REF_INFO);
    roundtrip::<GitSource>(GIT_SOURCE);
    roundtrip::<SkillInfo>(SKILL_INFO);
    roundtrip::<EmbeddedSource>(EMBEDDED_SOURCE);
    roundtrip::<Source>(r#"{"type":"local","path":"/abs/path"}"#);
    roundtrip::<SkillSource>(r#"{"type":"directory","path":"/abs/path"}"#);
    roundtrip::<Event<Updated>>(r#"{"id":"evt_1","type":"reference.updated","data":{}}"#);
}

#[test]
fn order_is_canonical_and_unknown_fields_are_removed() {
    let decoded: Info = serde_json::from_value(json!({
        "source": {"type": "local", "path": "/abs"},
        "path": "/abs",
        "name": "my-ref",
        "unknown": 1
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"name":"my-ref","path":"/abs","source":{"type":"local","path":"/abs"}}"#
    );
    assert!(decoded.description.is_none());
    assert!(decoded.hidden.is_none());
}

#[test]
fn optional_null_and_wrong_shapes_are_rejected() {
    for key in ["description", "hidden"] {
        let mut input: serde_json::Value = serde_json::from_str(
            r#"{"name":"r","path":"/p","source":{"type":"local","path":"/p"}}"#,
        )
        .unwrap();
        input[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
    for (key, value) in [
        ("source", json!({"type": "other"})),
        ("path", json!(1)),
        ("name", json!(1)),
    ] {
        let mut input: serde_json::Value = serde_json::from_str(
            r#"{"name":"r","path":"/p","source":{"type":"local","path":"/p"}}"#,
        )
        .unwrap();
        input[key] = value;
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
    for key in ["name", "path", "source"] {
        let mut input: serde_json::Value = serde_json::from_str(
            r#"{"name":"r","path":"/p","source":{"type":"local","path":"/p"}}"#,
        )
        .unwrap();
        input.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
}

#[test]
fn closed_literals_and_source_requirements_are_checked() {
    assert!(serde_json::from_str::<Source>(r#"{"type":"local"}"#).is_err());
    assert!(serde_json::from_str::<SkillSource>(r#"{"type":"directory"}"#).is_err());
    assert!(serde_json::from_str::<SkillSource>(r#"{"type":"url"}"#).is_err());
    assert!(serde_json::from_str::<SkillSource>(r#"{"type":"embedded"}"#).is_err());
    assert!(serde_json::from_str::<Source>(r#"{"type":"invalid","path":"/p"}"#).is_err());
}

#[test]
fn skill_info_optional_fields_omit_when_none() {
    let minimal = r#"{"name":"s","location":"/l","content":"c"}"#;
    roundtrip::<SkillInfo>(minimal);
    let decoded: SkillInfo = serde_json::from_value(json!({
        "content": "c",
        "location": "/l",
        "name": "s",
        "unknown": 1
    }))
    .unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), minimal);
    assert!(decoded.description.is_none());
    assert!(decoded.slash.is_none());
}
