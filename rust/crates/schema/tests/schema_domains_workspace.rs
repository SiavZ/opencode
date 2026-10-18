//! Purpose: verify workspace and worktree values and event codecs against Bun captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/workspace.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: all five event definitions have exact type and field order.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_workspace

use opencode_schema::event::Event;
use opencode_schema::workspace::{
    self, ConnectionStatus, Failed, Ready, Status, WorktreeFailed, WorktreeReady,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

#[test]
fn fixture_captured_from_bun_at_baseline() {
    // fixture: captured from bun at baseline, 2026-10-03, /tmp/p01home/workspace.ts.
    for status in ["connected", "connecting", "disconnected", "error"] {
        roundtrip::<ConnectionStatus>(&format!(r#"{{"workspaceID":"wrk","status":"{status}"}}"#));
    }
    roundtrip::<Event<Ready>>(r#"{"id":"evt_probe","type":"workspace.ready","data":{"name":""}}"#);
    roundtrip::<Event<Failed>>(
        r#"{"id":"evt_probe","type":"workspace.failed","data":{"message":"failure"}}"#,
    );
    roundtrip::<Event<Status>>(
        r#"{"id":"evt_probe","type":"workspace.status","data":{"workspaceID":"wrk_probe","status":"connected"}}"#,
    );
    roundtrip::<Event<WorktreeReady>>(
        r#"{"id":"evt_probe","type":"worktree.ready","data":{"name":"copy","branch":"main"}}"#,
    );
    roundtrip::<Event<WorktreeReady>>(
        r#"{"id":"evt_probe","type":"worktree.ready","data":{"name":""}}"#,
    );
    roundtrip::<Event<WorktreeFailed>>(
        r#"{"id":"evt_probe","type":"worktree.failed","data":{"message":"failure"}}"#,
    );
}

#[test]
fn optional_omission_and_null_rejection() {
    let ready: WorktreeReady = serde_json::from_str(r#"{"name":""}"#).unwrap();
    assert!(ready.branch.is_none());
    assert_eq!(serde_json::to_string(&ready).unwrap(), r#"{"name":""}"#);
    assert!(serde_json::from_str::<WorktreeReady>(r#"{"name":"","branch":null}"#).is_err());
    assert!(serde_json::from_str::<WorktreeReady>(r#"{"name":"","branch":1}"#).is_err());
    let ready: WorktreeReady =
        serde_json::from_str(r#"{"branch":"","extra":null,"name":""}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&ready).unwrap(),
        r#"{"name":"","branch":""}"#
    );
}

#[test]
fn required_fields_closed_status_and_ids_are_checked() {
    for input in [
        json!({}),
        json!({"status":"connected"}),
        json!({"workspaceID":"wrk"}),
        json!({"workspaceID":"wrong","status":"connected"}),
        json!({"workspaceID":"wrk","status":"invalid"}),
        json!({"workspaceID":"wrk","status":null}),
    ] {
        assert!(serde_json::from_value::<ConnectionStatus>(input).is_err());
    }
    assert!(serde_json::from_str::<Ready>("{}").is_err());
    assert!(serde_json::from_str::<Failed>(r#"{"message":null}"#).is_err());
    assert!(serde_json::from_str::<WorktreeFailed>(r#"{"message":2}"#).is_err());
    assert!(serde_json::from_str::<WorktreeReady>(r#"{"branch":"main"}"#).is_err());
    assert!(
        serde_json::from_str::<Event<Failed>>(
            r#"{"id":"evt_probe","type":"worktree.failed","data":{"message":"failure"}}"#
        )
        .is_err()
    );
}

#[test]
fn inventories_preserve_each_baseline_order() {
    assert_eq!(
        workspace::WORKSPACE_DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["workspace.ready", "workspace.failed", "workspace.status"]
    );
    assert_eq!(
        workspace::WORKTREE_DEFINITIONS
            .iter()
            .map(|d| d.r#type)
            .collect::<Vec<_>>(),
        ["worktree.ready", "worktree.failed"]
    );
    assert_eq!(workspace::DEFINITIONS.len(), 5);
    assert!(workspace::DEFINITIONS.iter().all(|d| d.durable.is_none()));
}
