//! Purpose: test fixtures for opencode-context: a real SQLite `Database` over rusqlite with the
//! baseline DDL for the tables context touches, and a minimal durable `EventStore` that commits
//! event rows plus the commit hook in one transaction. Fixture adapters, not production storage/event-store.
//! Scope: integration-test support.
//! Uses:
//! rust/crates/
//! |-- context/tests/support/events.rs
//! |-- event-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- rusqlite::Connection
//! Used by:
//! rust/crates/
//! |-- context/tests/epoch.rs
//! |-- context/tests/history.rs
//! `-- context/tests/support/events.rs
//! Invariants: every unit of work is one SQLite transaction, rollback on error.
//! Contract tests: none, fixture

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_event_api::{Definition, EventStore, PublishOptions};
use opencode_storage_api::{Access, Database, Row, Sql, StorageError, Value, WorkOutcome};
use rusqlite::Connection;

mod events;
pub use events::TestEvents;

/// Baseline DDL (packages/core/src/database/schema.gen.ts) for the tables context reads/writes.
pub const DDL: &str = r#"
CREATE TABLE `session` (`id` text PRIMARY KEY, `project_id` text NOT NULL, `slug` text NOT NULL,
  `directory` text NOT NULL, `title` text NOT NULL, `version` text NOT NULL,
  `time_created` integer NOT NULL, `time_updated` integer NOT NULL);
CREATE TABLE `event_sequence` (`aggregate_id` text PRIMARY KEY, `seq` integer NOT NULL, `owner_id` text);
CREATE TABLE `event` (`id` text PRIMARY KEY, `aggregate_id` text NOT NULL, `seq` integer NOT NULL,
  `type` text NOT NULL, `data` text NOT NULL,
  CONSTRAINT `fk_event_aggregate_id_event_sequence_aggregate_id_fk` FOREIGN KEY (`aggregate_id`) REFERENCES `event_sequence`(`aggregate_id`) ON DELETE CASCADE);
CREATE TABLE `session_context_epoch` (`session_id` text PRIMARY KEY, `baseline` text NOT NULL,
  `snapshot` text NOT NULL, `baseline_seq` integer NOT NULL,
  CONSTRAINT `fk_session_context_epoch_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE);
CREATE TABLE `session_message` (`id` text PRIMARY KEY, `session_id` text NOT NULL, `type` text NOT NULL,
  `seq` integer NOT NULL, `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL,
  CONSTRAINT `fk_session_message_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE);
CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
"#;

pub struct Conn(pub Connection);

fn map(error: rusqlite::Error) -> StorageError {
    let text = error.to_string();
    if text.contains("UNIQUE") {
        return StorageError::UniqueViolation(text);
    }
    StorageError::Other(text)
}

fn param(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Integer(i) => rusqlite::types::Value::Integer(*i),
        Value::Real(f) => rusqlite::types::Value::Real(*f),
        Value::Text(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
    }
}

impl Sql for Conn {
    fn execute(&self, sql: &str, params: &[Value]) -> opencode_storage_api::Result<usize> {
        let params: Vec<_> = params.iter().map(param).collect();
        self.0
            .execute(sql, rusqlite::params_from_iter(params))
            .map_err(map)
    }
    fn execute_batch(&self, sql: &str) -> opencode_storage_api::Result<()> {
        self.0.execute_batch(sql).map_err(map)
    }
    fn query(&self, sql: &str, params: &[Value]) -> opencode_storage_api::Result<Vec<Row>> {
        let mut stmt = self.0.prepare(sql).map_err(map)?;
        let columns: Arc<[String]> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let params: Vec<_> = params.iter().map(param).collect();
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params))
            .map_err(map)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(map)? {
            let values = (0..columns.len())
                .map(|i| {
                    Ok(
                        match row.get::<_, rusqlite::types::Value>(i).map_err(map)? {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(i) => Value::Integer(i),
                            rusqlite::types::Value::Real(f) => Value::Real(f),
                            rusqlite::types::Value::Text(s) => Value::Text(s),
                            rusqlite::types::Value::Blob(b) => Value::Blob(b),
                        },
                    )
                })
                .collect::<opencode_storage_api::Result<Vec<_>>>()?;
            out.push(Row::new(columns.clone(), values));
        }
        Ok(out)
    }
    fn last_insert_rowid(&self) -> i64 {
        self.0.last_insert_rowid()
    }
    fn savepoint_dyn(
        &self,
        work: &mut dyn FnMut(&dyn Sql) -> bool,
    ) -> opencode_storage_api::Result<bool> {
        self.0.execute_batch("SAVEPOINT sp").map_err(map)?;
        let ok = work(self);
        let sql = if ok {
            "RELEASE sp"
        } else {
            "ROLLBACK TO sp; RELEASE sp"
        };
        self.0.execute_batch(sql).map_err(map)?;
        Ok(ok)
    }
}

/// SQLite file database. Fixture standing in for opencode-storage-sqlite.
pub struct TestDb {
    pub conn: Mutex<Conn>,
}

impl TestDb {
    pub fn open(path: &Path) -> Arc<Self> {
        let conn = Connection::open(path).expect("open sqlite");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        Arc::new(Self {
            conn: Mutex::new(Conn(conn)),
        })
    }

    pub fn create(path: &Path) -> Arc<Self> {
        let db = Self::open(path);
        db.conn.lock().unwrap().execute_batch(DDL).unwrap();
        db
    }

    pub fn with<T>(&self, f: impl FnOnce(&dyn Sql) -> T) -> T {
        let conn = self.conn.lock().unwrap();
        f(&*conn)
    }
}

/// Run `work` in BEGIN..COMMIT, rolling back when `keep` says so.
fn transact<T>(
    conn: &Conn,
    work: impl FnOnce(&dyn Sql) -> (T, bool),
) -> opencode_storage_api::Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let (value, commit) = work(conn);
    if !commit {
        conn.execute_batch("ROLLBACK")?;
        return Ok(value);
    }
    if let Err(error) = conn.execute_batch("COMMIT") {
        if !conn.0.is_autocommit() {
            conn.execute_batch("ROLLBACK")?;
        }
        return Err(error);
    }
    Ok(value)
}

#[async_trait]
impl Database for TestDb {
    async fn run(
        &self,
        _access: Access,
        work: opencode_storage_api::Work,
    ) -> opencode_storage_api::Result<WorkOutcome> {
        let conn = self.conn.lock().unwrap();
        transact(&conn, |sql| {
            let outcome = work(sql);
            let commit = matches!(outcome, WorkOutcome::Commit(_));
            (outcome, commit)
        })
    }
    async fn close(&self) -> opencode_storage_api::Result<()> {
        Ok(())
    }
}

pub fn insert_session(db: &TestDb, id: &str) {
    db.with(|sql| {
        sql.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?, 'global', 's', '/p', 't', 'test', 0, 0)",
            &[Value::from(id)],
        )
        .unwrap();
    });
}

/// Append a durable event of an arbitrary type and (optionally) a projected message row at its seq.
pub async fn append(
    events: &TestEvents,
    session: &str,
    message: Option<(&str, &str, serde_json::Value)>,
) -> i64 {
    let def = Definition::durable("test.session.filler", 1, "sessionID");
    let seq = events
        .publish(
            &def,
            serde_json::json!({"sessionID": session}),
            PublishOptions::default(),
        )
        .await
        .unwrap()
        .durable
        .unwrap()
        .seq;
    if let Some((id, kind, data)) = message {
        events.db.with(|sql| {
            sql.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES (?, ?, ?, ?, 0, 0, ?)",
                &[Value::from(id), Value::from(session), Value::from(kind), Value::Integer(seq), Value::json(&data).unwrap()],
            )
            .unwrap();
        });
    }
    seq
}
