//! Purpose: JSON-row compatibility with the TypeScript baseline: the same durable publishes written
//! by Rust (`SqlEventStore`) and by TS (`EventV2` via bun) produce byte-identical `event` and
//! `event_sequence` rows (versioned `type`, compact JSON `data` in key order, seq, owner), and
//! Rust reads TS-written rows through `durable()`.
//! Scope: live test. Spawns `bun` from packages/core with HOME/XDG in a temp dir; skipped with a
//! message when bun or packages/core/node_modules are unavailable.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/lib.rs
//! |-- event-store/tests/support/mod.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- futures::StreamExt
//! |-- rusqlite::Connection::open
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches the real user data directory.
//! Contract tests: event-store/tests/rows.rs

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use futures::StreamExt;
use opencode_event_api::{EventStore, PublishOptions};
use opencode_event_store::SqlEventStore;
use opencode_storage_api::Database;
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::{Value as Json, json};
use support::SYNC;

fn repo() -> PathBuf {
    // OPENCODE_REPO lets the test run from an exported copy of `rust/` (shared-worktree isolation).
    std::env::var_os("OPENCODE_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .canonicalize()
        .unwrap()
}

fn available() -> bool {
    let core = repo().join("packages/core/node_modules/effect");
    let bun = Command::new("bun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !(bun && core.exists()) {
        eprintln!("SKIP rows: bun or packages/core/node_modules missing (live test not run)");
    }
    bun && core.exists()
}

/// Inputs exercising key order, unicode, escapes, numbers and nesting.
fn inputs() -> Vec<(String, Json)> {
    vec![
        (
            "evt_0000000000000000000000001a".into(),
            json!({"id": "agg_a", "text": "hello \"quoted\" ünïcode / \u{1f600}"}),
        ),
        (
            "evt_0000000000000000000000001b".into(),
            json!({"id": "agg_a", "text": "line\nbreak\ttab", "nested": {"z": 1, "a": [1.5, -2, 1e21, null, true], "m": "<>&"}}),
        ),
        (
            "evt_0000000000000000000000001c".into(),
            json!({"id": "agg_b", "text": ""}),
        ),
    ]
}

type RowTuple = (String, String, i64, String, String, i64, Option<String>);

fn rows(path: &Path) -> Vec<RowTuple> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.aggregate_id, e.seq, e.type, e.data, s.seq, s.owner_id FROM event e \
             JOIN event_sequence s ON s.aggregate_id = e.aggregate_id ORDER BY e.aggregate_id, e.seq",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn ts_publish(home: &Path, db: &Path) {
    let mut command = Command::new("bun");
    command
        .current_dir(repo().join("packages/core"))
        .arg(repo().join("rust/crates/event-store/tests/ts/publish_events.ts"))
        .arg(db)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("OPENCODE_DB");
    for (id, data) in inputs() {
        command.arg(id).arg(data.to_string());
    }
    let output = command.output().expect("spawn bun");
    assert!(
        output.status.success(),
        "bun failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn rust_rows_are_byte_identical_to_typescript_rows() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts_db = dir.path().join("ts.db");
    ts_publish(dir.path(), &ts_db);

    let rust_db = dir.path().join("rust.db");
    {
        let db: Arc<dyn Database> = Arc::new(SqliteDatabase::open(&rust_db).await.unwrap());
        let events = SqlEventStore::new(db.clone());
        for (id, data) in inputs() {
            events
                .publish(
                    &SYNC,
                    data,
                    PublishOptions {
                        id: Some(id),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        db.close().await.unwrap();
    }
    let expected = rows(&ts_db);
    assert_eq!(expected.len(), 3);
    assert_eq!(rows(&rust_db), expected);
    assert_eq!(expected[0].3, "test.sync.1");

    // Rust reads TS-written history through the durable stream.
    let db: Arc<dyn Database> = Arc::new(SqliteDatabase::open(&ts_db).await.unwrap());
    let events = SqlEventStore::new(db);
    events.register([&SYNC]);
    let read: Vec<_> = events
        .durable("agg_a", None)
        .take(2)
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(read[1].data, inputs()[1].1);
    assert_eq!(read[1].id, "evt_0000000000000000000000001b");
}

/// Known divergence (owner: storage-api `Value::json`): serde_json prints Rust-built floats `-0.0` and
/// `100.0`, JS `JSON.stringify` prints `0` and `100`. Data decoded from TS JSON keeps integers as
/// integers, so only floats constructed in Rust are affected. Pinned so a fix flips this test.
#[test]
fn rust_built_integral_floats_diverge_from_js_number_formatting() {
    let value = opencode_storage_api::Value::json(&json!({"a": -0.0, "b": 100.0})).unwrap();
    assert_eq!(
        value,
        opencode_storage_api::Value::Text(r#"{"a":-0.0,"b":100.0}"#.into())
    );
}

#[tokio::test]
async fn stored_data_is_compact_json_in_insertion_order() {
    let h = support::harness().await;
    h.events
        .publish(
            &SYNC,
            json!({"id": "agg", "text": "x", "nested": {"z": 1, "a": 2}}),
            Default::default(),
        )
        .await
        .unwrap();
    let row = h
        .query(
            "SELECT type, data FROM event WHERE aggregate_id = 'agg'",
            vec![],
        )
        .await;
    assert_eq!(row[0].get::<String>("type").unwrap(), "test.sync.1");
    assert_eq!(
        row[0].get::<String>("data").unwrap(),
        r#"{"id":"agg","text":"x","nested":{"z":1,"a":2}}"#
    );
}
