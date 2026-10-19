//! Purpose: native process-global SQLite handle. Owns one rusqlite connection on a dedicated worker
//! thread, applies baseline PRAGMAs and migrations at open, runs units of work with cancellation and
//! panic isolation, and closes cleanly.
//! Scope: process-global handle, one worker thread per handle, bounded job queue (backpressure).
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! |-- storage-sqlite/src/migration.rs
//! `-- storage-sqlite/src/transaction.rs
//! External:
//! |-- async_trait::async_trait
//! |-- migration::apply
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- oneshot::Receiver
//! |-- oneshot::Sender
//! |-- oneshot::channel
//! |-- rusqlite::Connection
//! |-- rusqlite::InterruptHandle
//! |-- rusqlite::OpenFlags
//! |-- tokio::select
//! |-- tokio::sync::mpsc
//! |-- tokio::sync::oneshot
//! |-- tokio_util::sync::CancellationToken::is_cancelled
//! |-- transaction::begin
//! |-- transaction::commit
//! |-- transaction::query
//! |-- transaction::rollback
//! `-- u64::from
//! Used by:
//! rust/crates/
//! `-- storage-sqlite/src/lib.rs
//! Invariants: PRAGMAs and order match packages/core/src/database/database.ts:26-31. A write is
//! acknowledged only after COMMIT. Cancellation interrupts only while the work closure runs, never during
//! COMMIT. A failed COMMIT is rolled back. A panicking closure rolls back and reports `Panicked`.
//! Contract tests: storage-sqlite/tests/transactions.rs, storage-sqlite/tests/ts_interop.rs

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_storage_api::{Access, Database, Result, StorageError, Work, WorkOutcome};
use rusqlite::{Connection, InterruptHandle, OpenFlags};
use tokio::sync::{mpsc, oneshot};

use crate::migration;
use crate::transaction::{self, Tx, map_error};

/// Jobs waiting for the worker. Senders wait (backpressure) when the queue is full.
const QUEUE_CAPACITY: usize = 256;

/// Baseline PRAGMAs, packages/core/src/database/database.ts:26-31, in order.
pub const BASELINE_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA busy_timeout = 5000",
    "PRAGMA cache_size = -64000",
    "PRAGMA foreign_keys = ON",
    "PRAGMA wal_checkpoint(PASSIVE)",
];

/// How to open a database.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Apply [`BASELINE_PRAGMAS`] (default true).
    pub pragmas: bool,
    /// Run `migration::apply` after the PRAGMAs (default true).
    pub migrate: bool,
    /// Open the file read-only (implies no migration and no WAL switch).
    pub read_only: bool,
    /// Override `busy_timeout` after the baseline PRAGMAs (tests use short timeouts).
    pub busy_timeout_ms: Option<u32>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            pragmas: true,
            migrate: true,
            read_only: false,
            busy_timeout_ms: None,
        }
    }
}

const PHASE_IDLE: u8 = 0;
const PHASE_WORK: u8 = 1;
const PHASE_COMMIT: u8 = 2;

/// Interrupt guard shared by the caller and the worker for one job.
struct JobControl {
    cancelled: Arc<AtomicBool>,
    phase: AtomicU8,
    gate: Mutex<()>,
}

struct Job {
    access: Access,
    work: Work,
    control: Arc<JobControl>,
    reply: oneshot::Sender<Result<WorkOutcome>>,
}

struct Inner {
    path: String,
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    interrupt: InterruptHandle,
    exited: Mutex<Option<oneshot::Receiver<()>>>,
}

/// Process-global SQLite database. Cheap to clone.
#[derive(Clone)]
pub struct SqliteDatabase {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SqliteDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteDatabase")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl SqliteDatabase {
    /// Open (creating if needed) with baseline PRAGMAs and migrations, like `Database.layerFromPath`.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, OpenOptions::default()).await
    }

    /// Fresh in-memory database with PRAGMAs and migrations.
    pub async fn open_in_memory() -> Result<Self> {
        Self::open_with(":memory:", OpenOptions::default()).await
    }

    pub async fn open_with(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let display = path.to_string_lossy().into_owned();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (sender, receiver) = mpsc::channel::<Job>(QUEUE_CAPACITY);
        let (exit_tx, exit_rx) = oneshot::channel();
        std::thread::Builder::new()
            .name(format!("opencode-sqlite:{display}"))
            .spawn(move || {
                let conn = match open_connection(&path, &options) {
                    Ok(conn) => conn,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        let _ = exit_tx.send(());
                        return;
                    }
                };
                if ready_tx.send(Ok(conn.get_interrupt_handle())).is_err() {
                    let _ = exit_tx.send(());
                    return;
                }
                worker(&conn, receiver);
                drop(conn.close());
                let _ = exit_tx.send(());
            })
            .map_err(|error| StorageError::Io(format!("spawn storage worker: {error}")))?;
        let interrupt = ready_rx
            .await
            .map_err(|_| StorageError::Panicked("storage worker failed to start".into()))??;
        Ok(Self {
            inner: Arc::new(Inner {
                path: display,
                sender: Mutex::new(Some(sender)),
                interrupt,
                exited: Mutex::new(Some(exit_rx)),
            }),
        })
    }

    pub fn path(&self) -> &str {
        &self.inner.path
    }
}

fn open_connection(path: &Path, options: &OpenOptions) -> Result<Connection> {
    let flags = if options.read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::default()
    };
    let conn = Connection::open_with_flags(path, flags).map_err(map_error)?;
    if options.pragmas {
        for pragma in BASELINE_PRAGMAS {
            if options.read_only
                && (pragma.contains("journal_mode") || pragma.contains("wal_checkpoint"))
            {
                continue;
            }
            pragma_with_retry(&conn, pragma)?;
        }
    }
    if let Some(ms) = options.busy_timeout_ms {
        conn.busy_timeout(std::time::Duration::from_millis(u64::from(ms)))
            .map_err(map_error)?;
    }
    if options.migrate && !options.read_only {
        migration::apply(&conn)?;
    }
    Ok(conn)
}

/// SQLite returns SQLITE_BUSY for `journal_mode = WAL` without calling the busy handler when two
/// connections race to switch a fresh file (lock-upgrade deadlock avoidance). Retry within the
/// baseline 5000 ms busy budget so concurrent opens of one path both succeed.
fn pragma_with_retry(conn: &Connection, pragma: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(5000);
    loop {
        match transaction::query(conn, pragma, &[]) {
            Ok(_) => return Ok(()),
            Err(StorageError::Busy(_)) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

fn worker(conn: &Connection, mut receiver: mpsc::Receiver<Job>) {
    while let Some(job) = receiver.blocking_recv() {
        let result = run_job(conn, job.access, job.work, &job.control);
        let _ = job.reply.send(result);
    }
}

fn run_job(
    conn: &Connection,
    access: Access,
    work: Work,
    control: &JobControl,
) -> Result<WorkOutcome> {
    if control.cancelled.load(Ordering::SeqCst) {
        return Err(StorageError::Cancelled);
    }
    let read = matches!(access, Access::Read { .. });
    let behavior = match &access {
        Access::Write(options) => options.behavior,
        Access::Read { .. } => opencode_storage_api::Behavior::Deferred,
    };
    if read {
        conn.execute_batch("PRAGMA query_only = ON")
            .map_err(map_error)?;
    }
    let result = run_unit(conn, behavior, work, control);
    if read {
        let _ = conn.execute_batch("PRAGMA query_only = OFF");
    }
    result
}

fn run_unit(
    conn: &Connection,
    behavior: opencode_storage_api::Behavior,
    work: Work,
    control: &JobControl,
) -> Result<WorkOutcome> {
    transaction::begin(conn, behavior)?;
    control.phase.store(PHASE_WORK, Ordering::SeqCst);
    let tx = Tx::new(conn, control.cancelled.clone());
    let outcome = catch_unwind(AssertUnwindSafe(|| work(&tx)));
    {
        // No interrupt may be issued from here on, so COMMIT is never interrupted.
        let _gate = control
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        control.phase.store(PHASE_COMMIT, Ordering::SeqCst);
    }
    match outcome {
        Err(panic) => {
            transaction::rollback(conn);
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(StorageError::Panicked(message))
        }
        Ok(WorkOutcome::Commit(value)) => {
            if conn.is_autocommit() {
                // The engine already rolled the transaction back (for example an interrupt that the work
                // swallowed). Never report success for writes that did not commit.
                return Err(if control.cancelled.load(Ordering::SeqCst) {
                    StorageError::Cancelled
                } else {
                    StorageError::Other("transaction was rolled back by the engine".into())
                });
            }
            transaction::commit(conn)?;
            Ok(WorkOutcome::Commit(value))
        }
        Ok(WorkOutcome::Rollback(error)) => {
            transaction::rollback(conn);
            Ok(WorkOutcome::Rollback(error))
        }
    }
}

#[async_trait]
impl Database for SqliteDatabase {
    async fn run(&self, access: Access, work: Work) -> Result<WorkOutcome> {
        let token = match &access {
            Access::Read { cancel } => cancel.clone(),
            Access::Write(options) => options.cancel.clone(),
        };
        if token
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        {
            return Err(StorageError::Cancelled);
        }
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(StorageError::Closed)?;
        let control = Arc::new(JobControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            phase: AtomicU8::new(PHASE_IDLE),
            gate: Mutex::new(()),
        });
        let (reply, mut response) = oneshot::channel();
        let job = Job {
            access,
            work,
            control: control.clone(),
            reply,
        };
        let sent = match &token {
            Some(token) => tokio::select! {
                sent = sender.send(job) => sent.map_err(|_| StorageError::Closed),
                () = token.cancelled() => return Err(StorageError::Cancelled),
            },
            None => sender.send(job).await.map_err(|_| StorageError::Closed),
        };
        drop(sender);
        sent?;
        let Some(token) = token else {
            return response.await.map_err(|_| StorageError::Closed)?;
        };
        tokio::select! {
            result = &mut response => return result.map_err(|_| StorageError::Closed)?,
            () = token.cancelled() => {}
        }
        control.cancelled.store(true, Ordering::SeqCst);
        {
            let _gate = control
                .gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if control.phase.load(Ordering::SeqCst) == PHASE_WORK {
                self.inner.interrupt.interrupt();
            }
        }
        response.await.map_err(|_| StorageError::Closed)?
    }

    async fn close(&self) -> Result<()> {
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(sender);
        let exited = self
            .inner
            .exited
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(exited) = exited {
            // Other clones may still hold no sender; queued jobs drain first, then the worker exits.
            let _ = exited.await;
        }
        Ok(())
    }
}

/// Inputs to the baseline database path rule (`database.ts:40-55`).
#[derive(Debug, Clone, Default)]
pub struct PathEnv {
    /// `OPENCODE_DB`.
    pub opencode_db: Option<String>,
    /// `OPENCODE_DISABLE_CHANNEL_DB`.
    pub disable_channel_db: Option<String>,
}

impl PathEnv {
    pub fn from_process() -> Self {
        Self {
            opencode_db: std::env::var("OPENCODE_DB").ok(),
            disable_channel_db: std::env::var("OPENCODE_DISABLE_CHANNEL_DB").ok(),
        }
    }
}

/// Database file for a data directory and installation channel, `Database.path()` in the baseline.
pub fn database_path(data_dir: &Path, channel: &str, env: &PathEnv) -> PathBuf {
    if let Some(db) = env.opencode_db.as_deref().filter(|db| !db.is_empty()) {
        if db == ":memory:" || Path::new(db).is_absolute() {
            return PathBuf::from(db);
        }
        return data_dir.join(db);
    }
    let disabled = matches!(env.disable_channel_db.as_deref(), Some("1" | "true"));
    if matches!(channel, "latest" | "beta" | "prod") || disabled {
        return data_dir.join("opencode.db");
    }
    let safe: String = channel
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    data_dir.join(format!("opencode-{safe}.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rules() {
        let data = Path::new("/data");
        let env = PathEnv::default();
        assert_eq!(
            database_path(data, "latest", &env),
            Path::new("/data/opencode.db")
        );
        assert_eq!(
            database_path(data, "dev", &env),
            Path::new("/data/opencode-dev.db")
        );
        assert_eq!(
            database_path(data, "feat/x y", &env),
            Path::new("/data/opencode-feat-x-y.db")
        );
        let disabled = PathEnv {
            disable_channel_db: Some("true".into()),
            ..PathEnv::default()
        };
        assert_eq!(
            database_path(data, "dev", &disabled),
            Path::new("/data/opencode.db")
        );
        let relative = PathEnv {
            opencode_db: Some("custom.db".into()),
            ..PathEnv::default()
        };
        assert_eq!(
            database_path(data, "dev", &relative),
            Path::new("/data/custom.db")
        );
        let absolute = PathEnv {
            opencode_db: Some("/tmp/a.db".into()),
            ..PathEnv::default()
        };
        assert_eq!(
            database_path(data, "dev", &absolute),
            Path::new("/tmp/a.db")
        );
        let memory = PathEnv {
            opencode_db: Some(":memory:".into()),
            ..PathEnv::default()
        };
        assert_eq!(database_path(data, "dev", &memory), Path::new(":memory:"));
        let empty = PathEnv {
            opencode_db: Some(String::new()),
            ..PathEnv::default()
        };
        assert_eq!(
            database_path(data, "prod", &empty),
            Path::new("/data/opencode.db")
        );
    }
}
