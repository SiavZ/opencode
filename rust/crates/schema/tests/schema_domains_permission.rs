//! Purpose: verify permission and saved-permission codecs against real baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/permission.rs
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
//! Contract tests: cargo test -p opencode-schema --test schema_domains_permission

use opencode_schema::event::Event;
use opencode_schema::permission::{
    self, Asked, Effect, Replied, Reply, Request, Ruleset, SavedInfo, Source,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-03, /tmp/p01home/permission.ts.
const REQUEST: &str = r#"{"id":"per_probe","sessionID":"ses_probe","action":"edit","resources":["src/**"],"save":[],"metadata":{"z":null,"a":[1,true]},"source":{"type":"tool","messageID":"not-branded","callID":""}}"#;
const MINIMAL: &str = r#"{"id":"per","sessionID":"ses","action":"","resources":[]}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Request>(REQUEST);
    roundtrip::<Request>(MINIMAL);
    roundtrip::<Source>(r#"{"type":"tool","messageID":"not-branded","callID":""}"#);
    roundtrip::<Ruleset>(
        r#"[{"action":"read","resource":"*","effect":"allow"},{"action":"read","resource":"secret","effect":"deny"},{"action":"write","resource":"*","effect":"ask"}]"#,
    );
    roundtrip::<SavedInfo>(r#"{"id":"","projectID":"","action":"edit","resource":"*"}"#);
    roundtrip::<Event<Asked>>(&format!(
        r#"{{"id":"evt_probe","type":"permission.v2.asked","data":{REQUEST}}}"#
    ));
    roundtrip::<Event<Replied>>(
        r#"{"id":"evt_probe","type":"permission.v2.replied","data":{"sessionID":"ses_probe","requestID":"per_probe","reply":"always"}}"#,
    );
}

#[test]
fn order_is_canonical_and_unknown_fields_are_removed() {
    let decoded: Request = serde_json::from_value(json!({
        "resources":[], "action":"", "id":"per", "sessionID":"ses", "unknown": 1
    }))
    .unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), MINIMAL);
    assert!(decoded.save.is_none());
    assert!(decoded.metadata.is_none());
    assert!(decoded.source.is_none());
}

#[test]
fn optional_null_and_wrong_shapes_are_rejected() {
    for key in ["save", "metadata", "source"] {
        let mut input: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
        input[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Request>(input).is_err(), "{key}");
    }
    for (key, value) in [
        ("save", json!([1])),
        ("metadata", json!([])),
        ("source", json!({"type":"other"})),
        ("resources", json!([false])),
        ("action", json!(1)),
        ("id", json!("x")),
        ("sessionID", json!("x")),
    ] {
        let mut input: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
        input[key] = value;
        assert!(serde_json::from_value::<Request>(input).is_err(), "{key}");
    }
    for key in ["id", "sessionID", "action", "resources"] {
        let mut input: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
        input.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Request>(input).is_err(), "{key}");
    }
}

#[test]
fn closed_literals_and_source_requirements_are_checked() {
    for value in ["once", "always", "reject"] {
        roundtrip::<Reply>(&format!("\"{value}\""));
    }
    for value in ["allow", "deny", "ask"] {
        roundtrip::<Effect>(&format!("\"{value}\""));
    }
    assert!(serde_json::from_str::<Reply>(r#""allow""#).is_err());
    assert!(serde_json::from_str::<Effect>(r#""always""#).is_err());
    assert!(serde_json::from_str::<Source>(r#"{"type":"tool","messageID":"m"}"#).is_err());
    assert!(
        serde_json::from_str::<Ruleset>(r#"[{"action":"read","resource":"*","effect":"invalid"}]"#)
            .is_err()
    );
    assert!(serde_json::from_str::<Event<Asked>>(r#"{"id":"evt_probe","type":"permission.v2.replied","data":{"id":"per","sessionID":"ses","action":"","resources":[]}}"#).is_err());
}

#[test]
fn inventory_preserves_baseline_order_and_durability() {
    assert_eq!(
        permission::DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["permission.v2.asked", "permission.v2.replied"]
    );
    assert!(permission::DEFINITIONS.iter().all(|d| d.durable.is_none()));
    roundtrip::<Ruleset>("[]");
}
