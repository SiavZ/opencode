//! Purpose: durable draft documents and content-addressed blobs in SQLite with the buffered
//! 500 ms write policy of the Electron host. Does not encode or decode renderer draft JSON.
//! Scope: one `drafts.sqlite` file (or `:memory:`) per desktop process.
//! Uses:
//! `-- none
//! External:
//! |-- hex::encode
//! |-- parking_lot::Mutex
//! |-- rusqlite::Connection
//! |-- rusqlite::Error
//! |-- rusqlite::OptionalExtension
//! |-- rusqlite::params
//! |-- serde_json::Error
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- sha2::Digest
//! |-- sha2::Sha256
//! |-- tokio::runtime::Handle
//! |-- tokio::task::AbortHandle
//! |-- tokio::task::JoinError
//! |-- tokio::task::JoinSet
//! |-- tokio::time::sleep
//! `-- tracing::error
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lib.rs
//! |-- desktop-host/src/migrate.rs
//! |-- desktop-host/tests/drafts.rs
//! `-- desktop-host/tests/migrate.rs
//! Invariants: tables `document(key TEXT PRIMARY KEY, value TEXT NOT NULL)` and
//! `blob(id TEXT PRIMARY KEY, data BLOB NOT NULL)` match the baseline byte for byte, reads see
//! pending (unflushed) writes, blob ids are lowercase sha256 hex, blobs unreferenced by any
//! document are deleted only at open, flush drains pending writes before the transaction (a
//! failed transaction drops them, as in the baseline), close and drop flush.
//! Contract tests: desktop-host/tests/drafts.rs

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinSet};

/// Delay between the first buffered write and its automatic flush (`draft-store.ts:55`).
pub const FLUSH_DELAY: Duration = Duration::from_millis(500);

/// File name of the draft database inside the user data directory (`ipc.ts:58`).
pub const DRAFTS_FILE: &str = "drafts.sqlite";

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS document (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS blob (id TEXT PRIMARY KEY, data BLOB NOT NULL);";

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("draft database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored document is not JSON. The baseline throws from `JSON.parse` while collecting
    /// blob references, so opening fails rather than deleting blobs it cannot account for.
    #[error("draft document {key:?} is not valid JSON: {source}")]
    CorruptDocument {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("draft flush task failed: {0}")]
    TimerJoin(#[from] tokio::task::JoinError),
    #[error("draft store is closed")]
    Closed,
}

/// Buffered SQLite draft store. Cheap to clone, all clones share one connection.
#[derive(Clone)]
pub struct DraftStore {
    inner: Arc<Inner>,
}

struct Inner {
    db: Mutex<Option<Connection>>,
    pending: Mutex<Vec<(String, Option<String>)>>,
    timer: Mutex<Option<AbortHandle>>,
    tasks: Mutex<JoinSet<()>>,
}

impl DraftStore {
    /// Opens (creating if needed) the database, enables WAL, creates the schema and deletes
    /// blobs that no document references (`draft-store.ts:16-36`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DraftError> {
        let db = Connection::open(path)?;
        // journal_mode returns a row, so it cannot go through execute_batch.
        db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        db.execute_batch(SCHEMA)?;
        collect_garbage(&db)?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Mutex::new(Some(db)),
                pending: Mutex::new(Vec::new()),
                timer: Mutex::new(None),
                tasks: Mutex::new(JoinSet::new()),
            }),
        })
    }

    /// Opens `<user_data>/drafts.sqlite`.
    pub fn open_in(user_data: impl AsRef<Path>) -> Result<Self, DraftError> {
        Self::open(user_data.as_ref().join(DRAFTS_FILE))
    }

    /// Returns the pending value when one is buffered, otherwise the stored value.
    pub fn get(&self, key: &str) -> Result<Option<String>, DraftError> {
        let guard = self.inner.db.lock();
        let db = guard.as_ref().ok_or(DraftError::Closed)?;
        if let Some(value) = self.inner.pending_value(key) {
            return Ok(value);
        }
        Ok(db
            .query_row("SELECT value FROM document WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Buffers a write (`Some`) or delete (`None`) and arms the flush timer when a tokio runtime
    /// is available. Without a runtime the write stays pending until [`DraftStore::flush`].
    pub fn set(&self, key: &str, value: Option<String>) {
        {
            let mut pending = self.inner.pending.lock();
            // Map semantics: re-setting a key keeps its original position, updates the value.
            match pending.iter_mut().find(|(existing, _)| existing == key) {
                Some(entry) => entry.1 = value,
                None => pending.push((key.to_string(), value)),
            }
        }
        self.schedule();
    }

    /// `draft-delete` IPC: buffers a delete (`ipc.ts:145`).
    pub fn delete(&self, key: &str) {
        self.set(key, None);
    }

    /// Stores bytes under their sha256 hex id, ignoring duplicates, and returns the id.
    pub fn put_blob(&self, data: &[u8]) -> Result<String, DraftError> {
        let id = hex::encode(Sha256::digest(data));
        let guard = self.inner.db.lock();
        let db = guard.as_ref().ok_or(DraftError::Closed)?;
        db.execute(
            "INSERT INTO blob (id, data) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            rusqlite::params![id, data],
        )?;
        Ok(id)
    }

    pub fn get_blob(&self, id: &str) -> Result<Option<Vec<u8>>, DraftError> {
        let guard = self.inner.db.lock();
        let db = guard.as_ref().ok_or(DraftError::Closed)?;
        Ok(db
            .query_row("SELECT data FROM blob WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Cancels the timer and writes every pending change in one transaction. Call on
    /// `before-quit` and on window `session-end` (`ipc.ts:61-63`).
    pub fn flush(&self) -> Result<(), DraftError> {
        if let Some(timer) = self.inner.timer.lock().take() {
            timer.abort();
        }
        self.inner.write_pending()
    }

    /// Cancels and joins the owned flush task before committing pending writes.
    /// Async shutdown hooks should prefer this over the synchronous compatibility API.
    pub async fn flush_and_join(&self) -> Result<(), DraftError> {
        if let Some(timer) = self.inner.timer.lock().take() {
            timer.abort();
        }
        let mut tasks = std::mem::take(&mut *self.inner.tasks.lock());
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                return Err(DraftError::TimerJoin(error));
            }
        }
        self.inner.write_pending()
    }

    /// Joined shutdown boundary. Admission must already be stopped by the host lifecycle.
    pub async fn close_and_join(&self) -> Result<(), DraftError> {
        self.flush_and_join().await?;
        self.close()
    }

    /// Flushes and closes the connection (`will-quit`). Later calls report [`DraftError::Closed`].
    pub fn close(&self) -> Result<(), DraftError> {
        self.flush()?;
        if let Some(db) = self.inner.db.lock().take() {
            db.close().map_err(|(_, error)| DraftError::Sqlite(error))?;
        }
        Ok(())
    }

    /// Number of buffered, not yet flushed writes.
    pub fn pending_len(&self) -> usize {
        self.inner.pending.lock().len()
    }

    fn schedule(&self) {
        let Ok(handle) = Handle::try_current() else {
            return;
        };
        let mut timer = self.inner.timer.lock();
        if timer.is_some() {
            return;
        }
        let mut tasks = self.inner.tasks.lock();
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                tracing::error!(%error, "draft flush task failed");
            }
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        *timer = Some(tasks.spawn_on(
            async move {
                tokio::time::sleep(FLUSH_DELAY).await;
                let Some(inner) = weak.upgrade() else { return };
                inner.timer.lock().take();
                if let Err(error) = inner.write_pending() {
                    tracing::error!(%error, "draft flush failed");
                }
            },
            &handle,
        ));
    }
}

impl Inner {
    fn pending_value(&self, key: &str) -> Option<Option<String>> {
        self.pending
            .lock()
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.clone())
    }

    fn write_pending(&self) -> Result<(), DraftError> {
        // Keep the DB lock across draining and committing so readers never observe the
        // transient gap where a buffered value is neither pending nor stored.
        let mut guard = self.db.lock();
        let writes = std::mem::take(&mut *self.pending.lock());
        if writes.is_empty() {
            return Ok(());
        }
        let db = guard.as_mut().ok_or(DraftError::Closed)?;
        let tx = db.transaction()?;
        for (key, value) in &writes {
            match value {
                None => tx.execute("DELETE FROM document WHERE key = ?1", [key])?,
                Some(value) => tx.execute(
                    "INSERT INTO document (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2",
                    [key, value],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.get_mut().take() {
            timer.abort();
        }
        if self.db.get_mut().is_some()
            && let Err(error) = self.write_pending()
        {
            tracing::error!(%error, "draft flush on drop failed");
        }
    }
}

/// Deletes every blob whose id is not referenced as `{"blob": {"id": "..."}}` anywhere inside a
/// stored document (`draft-store.ts:22-35`).
fn collect_garbage(db: &Connection) -> Result<(), DraftError> {
    let mut used = HashSet::new();
    let mut statement = db.prepare("SELECT key, value FROM document")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, value) = row?;
        let parsed: Value = serde_json::from_str(&value)
            .map_err(|source| DraftError::CorruptDocument { key, source })?;
        collect_refs(&parsed, &mut used);
    }
    let ids = db
        .prepare("SELECT id FROM blob")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in ids.iter().filter(|id| !used.contains(*id)) {
        db.execute("DELETE FROM blob WHERE id = ?1", [id])?;
    }
    Ok(())
}

fn collect_refs(value: &Value, used: &mut HashSet<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_refs(item, used)),
        Value::Object(map) => {
            if let Some(Value::String(id)) = map
                .get("blob")
                .and_then(Value::as_object)
                .and_then(|blob| blob.get("id"))
            {
                used.insert(id.clone());
            }
            map.values().for_each(|item| collect_refs(item, used));
        }
        _ => {}
    }
}

/// Snapshot of stored (flushed) documents, used by migration and tests.
pub fn stored_documents(store: &DraftStore) -> Result<HashMap<String, String>, DraftError> {
    let guard = store.inner.db.lock();
    let db = guard.as_ref().ok_or(DraftError::Closed)?;
    let mut statement = db.prepare("SELECT key, value FROM document")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
