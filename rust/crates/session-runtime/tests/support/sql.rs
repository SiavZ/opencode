//! Purpose: test support for p04 admission tests: open real migrated SQLite databases wired to the
//! real p02 `SqlEventStore` with the Session projectors installed, plus an execution double.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- event-store/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/projection.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tempfile::TempDir
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! `-- session-runtime/tests/admission_projection.rs
//! Invariants: every handle opens the same file with baseline PRAGMAs and migrations; `connect`
//! gives an independent handle (own connection, store and projector registry) like a second process.
//! `RecordingExecution` is a fixture standing in for SessionExecution (as in the baseline test layer).
//! Contract tests: session-runtime/tests/admission*.rs
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_event_store::SqlEventStore;
use opencode_session_api::operations::SessionExecution;
use opencode_session_api::{SessionID, SessionResult};
use opencode_session_runtime::projection::LocalSessionEvents;
use opencode_storage_api::{Database, DatabaseExt, Sql, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;

/// A migrated database file plus the Session event wiring.
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub db: Arc<dyn Database>,
    pub store: Arc<SqlEventStore>,
    pub events: Arc<LocalSessionEvents>,
}

/// Opens a fresh migrated SQLite file and installs the Session projectors.
pub async fn open() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let (db, store, events) = connect(&path).await;
    Harness {
        dir,
        db,
        store,
        events,
    }
}

/// Opens another process-like handle (own connection, own projector registry) on the same file.
pub async fn connect(
    path: &std::path::Path,
) -> (
    Arc<dyn Database>,
    Arc<SqlEventStore>,
    Arc<LocalSessionEvents>,
) {
    let db: Arc<dyn Database> = Arc::new(SqliteDatabase::open(path).await.unwrap());
    let store = Arc::new(SqlEventStore::new(db.clone()));
    let events = Arc::new(LocalSessionEvents::install(db.clone(), store.clone()));
    (db, store, events)
}

impl Harness {
    pub fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("opencode.db")
    }

    /// Baseline test `setup`: global project and one Session row.
    pub async fn seed_session(&self, id: &str) {
        let id = id.to_owned();
        self.db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(
                    "INSERT INTO project (id, worktree, sandboxes, time_created, time_updated) VALUES ('global', '/project', '[]', 0, 0) ON CONFLICT DO NOTHING",
                    &[],
                )?;
                tx.execute(
                    "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?, 'global', 'test', '/project', 'test', 'test', 1, 1) ON CONFLICT DO NOTHING",
                    &[id.into()],
                )
            })
            .await
            .unwrap();
    }

    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Vec<opencode_storage_api::Row> {
        let sql = sql.to_owned();
        self.db
            .read(move |tx: &dyn Sql| tx.query(&sql, &params))
            .await
            .unwrap()
    }

    pub async fn count(&self, sql: &str, params: Vec<Value>) -> i64 {
        self.query(sql, params).await[0].get_index(0).unwrap()
    }
}

/// Execution double recording wake/resume/interrupt calls (fixture, like the baseline test layer).
#[derive(Default)]
pub struct RecordingExecution {
    pub wakes: Mutex<Vec<SessionID>>,
    pub resumes: Mutex<Vec<SessionID>>,
    pub interrupts: Mutex<Vec<SessionID>>,
    pub active: Mutex<BTreeSet<SessionID>>,
}

#[async_trait]
impl SessionExecution for RecordingExecution {
    async fn active(&self) -> BTreeSet<SessionID> {
        self.active.lock().unwrap().clone()
    }
    async fn resume(&self, session_id: &SessionID) -> SessionResult<()> {
        self.resumes.lock().unwrap().push(session_id.clone());
        Ok(())
    }
    async fn wake(&self, session_id: &SessionID) {
        self.wakes.lock().unwrap().push(session_id.clone());
    }
    async fn interrupt(&self, session_id: &SessionID) {
        self.interrupts.lock().unwrap().push(session_id.clone());
    }
}
