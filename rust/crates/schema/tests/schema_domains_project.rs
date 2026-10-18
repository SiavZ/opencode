//! Purpose: verify project metadata, copy codecs and events against baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/project.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are Bun output, JS trim and optional null rejection are preserved.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_project

use opencode_schema::event::Event;
use opencode_schema::ids::ProjectCopyStrategyID;
use opencode_schema::project::{
    self, Commands, Copy, CreateInput, DirectoriesUpdated, Icon, Info, RemoveInput, StrategyID,
    Time, Updated,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-03, /tmp/p01home/project.ts.
const INFO: &str = r#"{"id":"global","worktree":"","vcs":"git","name":"","icon":{"url":"u","override":"","color":"x"},"commands":{"start":""},"time":{"created":1,"updated":2,"initialized":3},"sandboxes":["relative","/tmp/a"]}"#;
const MINIMAL: &str =
    r#"{"id":"","worktree":"relative","time":{"created":0,"updated":0},"sandboxes":[]}"#;
const CREATE: &str = r#"{"projectID":"global","strategy":"clone","sourceDirectory":"","directory":"relative","name":""}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Info>(INFO);
    roundtrip::<Info>(MINIMAL);
    roundtrip::<Icon>(r#"{"url":"u","override":"","color":"x"}"#);
    roundtrip::<Icon>("{}");
    roundtrip::<Commands>(r#"{"start":""}"#);
    roundtrip::<Commands>("{}");
    roundtrip::<Time>(r#"{"created":1,"updated":2,"initialized":3}"#);
    roundtrip::<CreateInput>(CREATE);
    roundtrip::<RemoveInput>(r#"{"projectID":"","directory":"relative","force":false}"#);
    roundtrip::<Copy>(r#"{"directory":""}"#);
    roundtrip::<Event<Updated>>(&format!(
        r#"{{"id":"evt_probe","type":"project.updated","data":{INFO}}}"#
    ));
    roundtrip::<Event<DirectoriesUpdated>>(
        r#"{"id":"evt_probe","type":"project.directories.updated","data":{"projectID":"global"}}"#,
    );
}

#[test]
fn strategy_decode_matches_javascript_trim() {
    let mut input: serde_json::Value = serde_json::from_str(CREATE).unwrap();
    input["strategy"] = json!("\u{feff}  clone \n");
    let decoded: CreateInput = serde_json::from_value(input).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), CREATE);
    for value in ["", " ", "\u{feff}", "\t\n\r\u{a0}\u{2028}\u{2029}"] {
        assert!(serde_json::from_value::<StrategyID>(json!(value)).is_err());
    }
    for value in ["\u{0085}", "\u{180e}", "\u{200b}"] {
        let decoded: StrategyID = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(decoded.as_str(), value);
    }
    let decoded: StrategyID = serde_json::from_value(json!("\u{2028}x\u{2029}")).unwrap();
    assert_eq!(decoded.into_inner(), "x");
    assert!(serde_json::from_str::<StrategyID>("null").is_err());
    assert!(serde_json::from_str::<StrategyID>("5").is_err());
}

#[test]
fn centralized_strategy_brand_preserves_plain_string_encoding() {
    let value = ProjectCopyStrategyID::new("\u{feff} copy \u{2028}\u{2029}").unwrap();
    assert_eq!(value.as_str(), "copy");
    assert_eq!(serde_json::to_string(&value).unwrap(), r#""copy""#);
    assert_eq!(value.to_string(), "copy");
    assert_eq!(value.as_ref(), "copy");
    let compatible: StrategyID = value.clone();
    assert_eq!(compatible, value);
    roundtrip::<ProjectCopyStrategyID>(r#""copy""#);
    assert!(ProjectCopyStrategyID::new("\u{feff}\u{2028}\u{2029}").is_err());
}

#[test]
fn optional_fields_omit_missing_and_reject_null() {
    for key in ["vcs", "name", "icon", "commands"] {
        let mut input: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
    for key in ["url", "override", "color"] {
        assert!(serde_json::from_value::<Icon>(json!({key:null})).is_err());
    }
    assert!(serde_json::from_str::<Commands>(r#"{"start":null}"#).is_err());
    assert!(
        serde_json::from_str::<Time>(r#"{"created":0,"updated":0,"initialized":null}"#).is_err()
    );
    let mut input: serde_json::Value = serde_json::from_str(CREATE).unwrap();
    input["name"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<CreateInput>(input).is_err());
    let mut input: serde_json::Value = serde_json::from_str(CREATE).unwrap();
    input.as_object_mut().unwrap().remove("name");
    let decoded: CreateInput = serde_json::from_value(input).unwrap();
    assert!(decoded.name.is_none());
}

#[test]
fn validates_time_vcs_and_required_copy_fields() {
    for value in [
        json!(-1),
        json!(0.5),
        json!(9007199254740992_u64),
        json!("0"),
    ] {
        for key in ["created", "updated", "initialized"] {
            let mut input = json!({"created":0,"updated":0});
            input[key] = value.clone();
            assert!(serde_json::from_value::<Time>(input).is_err());
        }
    }
    let mut input: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
    input["vcs"] = json!("svn");
    assert!(serde_json::from_value::<Info>(input).is_err());
    assert!(serde_json::from_str::<RemoveInput>(r#"{"projectID":"p","directory":"d"}"#).is_err());
    assert!(
        serde_json::from_str::<RemoveInput>(r#"{"projectID":"p","directory":"d","force":1}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<Copy>(r#"{"directory":null}"#).is_err());
}

#[test]
fn inventory_and_unknown_fields_match_baseline() {
    assert_eq!(
        project::DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["project.updated", "project.directories.updated"]
    );
    assert_eq!(project::PROJECT_DEFINITIONS.len(), 1);
    assert_eq!(project::DIRECTORIES_DEFINITIONS.len(), 1);
    assert!(project::DEFINITIONS.iter().all(|d| d.durable.is_none()));
    let decoded: Icon = serde_json::from_str(r#"{"unknown":"value"}"#).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), "{}");
}
