//! Purpose: shared harness for event-store integration tests: a real migrated SQLite database in a
//! temp dir, test definitions mirroring packages/core/test/event.test.ts, and row helpers.
//! Scope: test support (live implementation, no fixtures).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/lib.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! `-- tempfile::tempdir
//! Used by:
//! rust/crates/
//! |-- event-store/tests/commit.rs
//! |-- event-store/tests/owners.rs
//! |-- event-store/tests/replay.rs
//! |-- event-store/tests/rows.rs
//! `-- event-store/tests/streams.rs
//! Invariants: every harness owns its temp directory; nothing touches user data.
//! Contract tests: event-store/tests/

#![allow(dead_code)]

use std::sync::Arc;

use opencode_event_api::{Definition, EventError, SerializedEvent, versioned_type};
use opencode_event_store::{Options, SqlEventStore};
use opencode_storage_api::{Database, DatabaseExt, Row, Sql, StorageError, Value};
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::{Value as Json, json};

pub const MESSAGE: Definition = Definition::ephemeral("test.message");
pub const SYNC: Definition = Definition::durable("test.sync", 1, "id");
pub const SENT: Definition = Definition::durable("test.sent", 1, "messageID");
pub const VERSIONED: Definition = Definition::durable("test.versioned", 2, "id");
/// Stand-in for `SessionV1.Event.MessageRemoved` (`session.v1.message.removed`, aggregate sessionID).
pub const REMOVED: Definition = Definition::durable("message.removed", 1, "sessionID");

pub struct Harness {
    pub _dir: tempfile::TempDir,
    pub db: Arc<dyn Database>,
    pub events: SqlEventStore,
}

pub async fn harness() -> Harness {
    harness_with(Options::default()).await
}

pub async fn harness_with(options: Options) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let db: Arc<dyn Database> = Arc::new(
        SqliteDatabase::open(dir.path().join("opencode.db"))
            .await
            .expect("open db"),
    );
    let events = SqlEventStore::with_options(db.clone(), options);
    events.register([&SYNC, &SENT, &VERSIONED, &REMOVED]);
    Harness {
        _dir: dir,
        db,
        events,
    }
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Unique aggregate/event IDs per test run.
pub fn id(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("{prefix}_{}_{n}", std::process::id())
}

pub fn removed(session: &str, text: &str) -> Json {
    json!({ "sessionID": session, "messageID": format!("msg_{text}") })
}

pub fn serialized(id: &str, aggregate: &str, seq: i64, data: Json) -> SerializedEvent {
    SerializedEvent {
        id: id.to_owned(),
        event_type: versioned_type(REMOVED.event_type, 1),
        seq,
        aggregate_id: aggregate.to_owned(),
        data,
    }
}

pub fn message(error: &EventError) -> String {
    error.to_string()
}

impl Harness {
    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Vec<Row> {
        let sql = sql.to_owned();
        self.db
            .read(move |tx: &dyn Sql| tx.query(&sql, &params))
            .await
            .expect("query")
    }

    pub async fn exec(&self, sql: &str) {
        let sql = sql.to_owned();
        self.db
            .transaction(Default::default(), move |tx: &dyn Sql| {
                tx.execute_batch(&sql)?;
                Ok::<_, StorageError>(())
            })
            .await
            .expect("exec");
    }

    pub async fn seqs(&self, aggregate: &str) -> Vec<i64> {
        self.query(
            "SELECT seq FROM event WHERE aggregate_id = ? ORDER BY seq",
            vec![aggregate.into()],
        )
        .await
        .iter()
        .map(|row| row.get::<i64>("seq").unwrap())
        .collect()
    }

    pub async fn sequence(&self, aggregate: &str) -> Option<(i64, Option<String>)> {
        self.query(
            "SELECT seq, owner_id FROM event_sequence WHERE aggregate_id = ?",
            vec![aggregate.into()],
        )
        .await
        .first()
        .map(|row| (row.get("seq").unwrap(), row.get("owner_id").unwrap()))
    }
}
