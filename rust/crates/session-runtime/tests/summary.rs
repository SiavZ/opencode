//! Purpose: fixture tests for legacy file-change summaries (`SessionSummary.summarize/diff/
//! computeDiff`, `unquoteGitPath`) over a recording `Snapshots` fixture.
//! Scope: test-only; real SQLite + legacy projectors.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/summary.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: `FixtureSnapshots` is a labeled fixture returning canned diffs.
//! Contract tests: session-runtime/tests/summary.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_platform_api::snapshot::{DiffStatus, FileDiff, SnapshotError, Snapshots};
use opencode_session_runtime::summary::{Summary, unquote_git_path};
use serde_json::json;
use support::{SESSION, setup};

#[derive(Debug, Default)]
struct FixtureSnapshots {
    calls: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl Snapshots for FixtureSnapshots {
    async fn capture(&self) -> Option<String> {
        None
    }
    async fn files(&self, _: &str, _: &str) -> Result<Vec<String>, SnapshotError> {
        Ok(vec![])
    }
    async fn diff(
        &self,
        from: &str,
        to: &str,
        _: Option<u32>,
        _: Option<&[String]>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        self.calls.lock().unwrap().push((from.into(), to.into()));
        Ok(vec![FileDiff {
            file: Some("\"caf\\303\\251.txt\"".into()),
            patch: None,
            additions: 2,
            deletions: 1,
            status: Some(DiffStatus::Modified),
        }])
    }
    async fn preview(
        &self,
        _: &BTreeMap<String, String>,
        _: Option<u32>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        Ok(vec![])
    }
    async fn restore(&self, _: &BTreeMap<String, String>) -> Result<(), SnapshotError> {
        Ok(())
    }
    async fn checkout(&self, _: &str) -> Result<(), SnapshotError> {
        Ok(())
    }
}

#[test]
fn unquote_git_path_decodes_octal_and_escapes() {
    assert_eq!(unquote_git_path("\"caf\\303\\251.txt\""), "café.txt");
    assert_eq!(unquote_git_path("\"a\\tb\\\"c\""), "a\tb\"c");
    assert_eq!(unquote_git_path("plain.txt"), "plain.txt");
}

#[tokio::test]
async fn summarize_writes_user_diffs_from_step_snapshots() {
    let ctx = setup().await;
    let store = ctx.legacy.store();
    let put_message = |value: serde_json::Value| serde_json::from_value(value).unwrap();
    store.update_message(&put_message(json!({ "id": "msg_u", "sessionID": SESSION, "role": "user", "time": { "created": 1 }, "summary": { "title": "t", "diffs": [] } }))).await.unwrap();
    store.update_message(&put_message(json!({ "id": "msg_a", "sessionID": SESSION, "role": "assistant", "parentID": "msg_u", "time": { "created": 2 } }))).await.unwrap();
    store.update_part(&put_message(json!({ "id": "prt_1", "messageID": "msg_a", "sessionID": SESSION, "type": "step-start", "snapshot": "s1" }))).await.unwrap();
    store.update_part(&put_message(json!({ "id": "prt_2", "messageID": "msg_a", "sessionID": SESSION, "type": "step-finish", "snapshot": "s2", "reason": "stop", "cost": 0, "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }))).await.unwrap();
    let snapshots = Arc::new(FixtureSnapshots::default());
    let summary = Summary {
        store: store.clone(),
        snapshots: snapshots.clone(),
        snapshot: None,
    };
    summary.summarize(SESSION, "msg_u").await.unwrap();
    assert_eq!(
        *snapshots.calls.lock().unwrap(),
        vec![("s1".to_string(), "s2".to_string())]
    );
    let session = store.get(SESSION).await.unwrap();
    assert_eq!(
        session["summary"],
        json!({ "additions": 0, "deletions": 0, "files": 0 })
    );
    let user = &ctx.messages().await[0];
    assert_eq!(user.info["summary"]["title"], "t");
    assert_eq!(user.info["summary"]["diffs"][0]["additions"], 2);
    let diffs = summary.diff(SESSION, Some("msg_u")).await.unwrap();
    assert_eq!(diffs[0]["file"], "café.txt");
    assert!(summary.diff(SESSION, None).await.unwrap().is_empty());
    assert!(
        summary
            .diff(SESSION, Some("msg_a"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn snapshot_disabled_only_resets_summary() {
    let ctx = setup().await;
    let snapshots = Arc::new(FixtureSnapshots::default());
    let summary = Summary {
        store: ctx.legacy.store().clone(),
        snapshots: snapshots.clone(),
        snapshot: Some(false),
    };
    summary.summarize(SESSION, "msg_missing").await.unwrap();
    assert!(snapshots.calls.lock().unwrap().is_empty());
    assert_eq!(
        ctx.legacy.store().get(SESSION).await.unwrap()["summary"]["files"],
        0
    );
}
