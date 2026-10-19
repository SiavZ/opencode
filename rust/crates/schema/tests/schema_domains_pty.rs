//! Purpose: verify pty metadata, inputs and events against baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/pty.rs
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
//! Invariants: fixtures are Bun output, JS trim and optional null rejection are preserved.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_pty

use opencode_schema::event::Event;
use opencode_schema::ids::PtyID;
use opencode_schema::pty::{
    self, ConnectToken, CreateInput, Deleted, Exited, Info, Size, Status, UpdateInput, Updated,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-15, /tmp/p01home/pty.ts.
const INFO: &str = r#"{"id":"pty_1","title":"t","command":"c","args":["a"],"cwd":"/","status":"running","pid":123,"exitCode":0}"#;
const INFO_NO_EXIT: &str =
    r#"{"id":"pty_2","title":"t","command":"c","args":[],"cwd":"/","status":"exited","pid":456}"#;
const CREATE: &str = r#"{"command":"c","args":["a"],"cwd":"/","title":"t","env":{"A":"B"}}"#;
const UPDATE: &str = r#"{"title":"t","size":{"rows":1,"cols":2}}"#;
const CONNECT_TOKEN: &str = r#"{"ticket":"tok","expires_in":100}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Info>(INFO);
    roundtrip::<Info>(INFO_NO_EXIT);
    roundtrip::<CreateInput>(CREATE);
    roundtrip::<Size>(r#"{"rows":1,"cols":2}"#);
    roundtrip::<UpdateInput>(UPDATE);
    roundtrip::<ConnectToken>(CONNECT_TOKEN);
    roundtrip::<Event<Updated>>(&format!(
        r#"{{"id":"evt_probe","type":"pty.updated","data":{}}}"#,
        r#"{"info":{"id":"pty_1","title":"t","command":"c","args":["a"],"cwd":"/","status":"running","pid":123}}"#
    ));
    roundtrip::<Event<Exited>>(
        r#"{"id":"evt_probe","type":"pty.exited","data":{"id":"pty_1","exitCode":1}}"#,
    );
    roundtrip::<Event<Deleted>>(r#"{"id":"evt_probe","type":"pty.deleted","data":{"id":"pty_1"}}"#);
}

#[test]
fn optional_fields_omit_missing_and_reject_null() {
    {
        let key = "exitCode";
        let mut input: serde_json::Value = serde_json::from_str(INFO_NO_EXIT).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Info>(input).is_err(), "{key}");
    }
    for key in ["command", "args", "cwd", "title", "env"] {
        let mut input: serde_json::Value = serde_json::from_str(r#"{"args":["a"]}"#).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<CreateInput>(input).is_err(),
            "{key}"
        );
    }
    for key in ["title", "size"] {
        let mut input: serde_json::Value = serde_json::from_str(r#"{"title":"t"}"#).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<UpdateInput>(input).is_err(),
            "{key}"
        );
    }
    let mut input: serde_json::Value = serde_json::from_str(CREATE).unwrap();
    input.as_object_mut().unwrap().remove("env");
    let decoded: CreateInput = serde_json::from_value(input).unwrap();
    assert!(decoded.env.is_none());
}

#[test]
fn validates_pid_and_positive_int() {
    for value in [
        json!(-1),
        json!(0.5),
        json!(9007199254740992_u64),
        json!("0"),
    ] {
        let mut input = json!({"id":"pty_1","title":"t","command":"c","args":[],"cwd":"/","status":"running","pid":123});
        input["pid"] = value.clone();
        assert!(
            serde_json::from_value::<Info>(input).is_err(),
            "pid: {value}"
        );
    }
    for value in [json!(0), json!(-1), json!(0.5)] {
        let mut input = json!({"rows":1,"cols":2});
        input["rows"] = value.clone();
        assert!(
            serde_json::from_value::<Size>(input).is_err(),
            "rows: {value}"
        );
        let mut input2 = json!({"rows":1,"cols":2});
        input2["cols"] = value.clone();
        assert!(
            serde_json::from_value::<Size>(input2).is_err(),
            "cols: {value}"
        );
    }
    for value in [json!(0), json!(-1)] {
        let mut input = json!({"ticket":"t","expires_in":100});
        input["expires_in"] = value.clone();
        assert!(
            serde_json::from_value::<ConnectToken>(input).is_err(),
            "expires_in: {value}"
        );
    }
}

#[test]
fn status_enum_matches_baseline() {
    assert_eq!(
        serde_json::to_string(&Status::Running).unwrap(),
        r#""running""#
    );
    assert_eq!(
        serde_json::to_string(&Status::Exited).unwrap(),
        r#""exited""#
    );
    assert!(serde_json::from_str::<Status>(r#""invalid""#).is_err());
}

#[test]
fn inventory_matches_baseline() {
    assert_eq!(
        pty::DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["pty.created", "pty.updated", "pty.exited", "pty.deleted"]
    );
    assert!(pty::DEFINITIONS.iter().all(|d| d.durable.is_none()));
}

#[test]
fn pty_id_prefix_check() {
    assert!(PtyID::new("pty_123").is_ok());
    assert!(PtyID::new("invalid").is_err());
    let id = PtyID::new("pty_test").unwrap();
    assert_eq!(id.as_str(), "pty_test");
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""pty_test""#);
}
