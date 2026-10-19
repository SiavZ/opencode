//! Purpose: verify filesystem and watcher DTOs against baseline JSON captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/filesystem.rs
//! External:
//! |-- serde::Serialize
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: Schema.Class decode produces validated DTOs and no extra defaults.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_filesystem

use opencode_schema::event::Event;
use opencode_schema::filesystem::{
    self, Edited, Entry, FindInput, Match, Submatch, WatcherUpdated,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-03, /tmp/p01home/filesystem.ts.
const MATCH: &str = r#"{"entry":{"path":"relative","type":"file"},"line":1,"offset":0,"text":"x","submatches":[{"text":"x","start":3,"end":0}]}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Entry>(r#"{"path":"relative","type":"file"}"#);
    roundtrip::<Entry>(r#"{"path":"/absolute-is-allowed","type":"directory"}"#);
    // No start <= end constraint exists in the baseline.
    roundtrip::<Submatch>(r#"{"text":"x","start":3,"end":0}"#);
    roundtrip::<Match>(MATCH);
    roundtrip::<FindInput>(r#"{"query":"","type":"directory","limit":5}"#);
    roundtrip::<FindInput>(r#"{"query":""}"#);
    roundtrip::<Event<Edited>>(r#"{"id":"evt_probe","type":"file.edited","data":{"file":""}}"#);
    for event in ["add", "change", "unlink"] {
        roundtrip::<Event<WatcherUpdated>>(&format!(
            r#"{{"id":"evt_probe","type":"file.watcher.updated","data":{{"file":"","event":"{event}"}}}}"#
        ));
    }
}

#[test]
fn class_input_omits_absent_rejects_null_and_strips_unknown_keys() {
    let decoded: FindInput = serde_json::from_str(r#"{"unknown":null,"query":""}"#).unwrap();
    assert!(decoded.entry_type.is_none());
    assert!(decoded.limit.is_none());
    assert_eq!(serde_json::to_string(&decoded).unwrap(), r#"{"query":""}"#);
    for key in ["type", "limit"] {
        assert!(serde_json::from_value::<FindInput>(json!({"query":"",key:null})).is_err());
    }
    assert!(serde_json::from_str::<FindInput>("{}").is_err());
    assert!(serde_json::from_str::<FindInput>(r#"{"query":null}"#).is_err());
    assert!(serde_json::from_str::<FindInput>(r#"{"query":"","type":"symlink"}"#).is_err());
}

#[test]
fn numeric_constraints_match_schema_safe_integer_bounds() {
    for value in [
        json!(-1),
        json!(0.5),
        json!(9007199254740992_u64),
        json!("1"),
        json!(null),
    ] {
        for key in ["start", "end"] {
            let mut input = json!({"text":"", "start":0, "end":0});
            input[key] = value.clone();
            assert!(serde_json::from_value::<Submatch>(input).is_err());
        }
        for key in ["line", "offset"] {
            let mut input: serde_json::Value = serde_json::from_str(MATCH).unwrap();
            input[key] = value.clone();
            assert!(serde_json::from_value::<Match>(input).is_err());
        }
        assert!(serde_json::from_value::<FindInput>(json!({"query":"", "limit":value})).is_err());
    }
    assert!(serde_json::from_str::<FindInput>(r#"{"query":"","limit":0}"#).is_err());
    let mut input: serde_json::Value = serde_json::from_str(MATCH).unwrap();
    input["line"] = json!(0);
    assert!(serde_json::from_value::<Match>(input).is_err());
    roundtrip::<Submatch>(r#"{"text":"","start":9007199254740991,"end":0}"#);
    let decoded: FindInput = serde_json::from_str(r#"{"query":"","limit":1.0}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"query":"","limit":1}"#
    );
}

#[test]
fn closed_types_and_required_fields_are_validated() {
    assert!(serde_json::from_str::<Entry>(r#"{"path":"x","type":"other"}"#).is_err());
    assert!(serde_json::from_str::<Entry>(r#"{"type":"file"}"#).is_err());
    assert!(serde_json::from_str::<Submatch>(r#"{"text":"","start":0}"#).is_err());
    assert!(serde_json::from_str::<WatcherUpdated>(r#"{"file":"","event":"delete"}"#).is_err());
    assert!(serde_json::from_str::<Edited>(r#"{"file":null}"#).is_err());
    assert!(
        serde_json::from_str::<Event<Edited>>(
            r#"{"id":"evt_probe","type":"file.watcher.updated","data":{"file":""}}"#
        )
        .is_err()
    );
}

#[test]
fn definitions_match_baseline_order() {
    assert_eq!(
        filesystem::DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["file.edited", "file.watcher.updated"]
    );
    assert_eq!(filesystem::FILE_DEFINITIONS.len(), 1);
    assert_eq!(filesystem::WATCHER_DEFINITIONS.len(), 1);
    assert!(filesystem::DEFINITIONS.iter().all(|d| d.durable.is_none()));
}
