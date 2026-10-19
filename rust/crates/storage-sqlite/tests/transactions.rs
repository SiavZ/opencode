//! Purpose: conformance tests for SqliteDatabase units of work: commit, rollback, savepoints, panics,
//! cancellation, busy contention, failed commit, read-only access and close.
//! Scope: live tests against real SQLite files in temp dirs (no fixtures).
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- i64::MAX
//! |-- tempfile::tempdir
//! |-- tokio::join
//! |-- tokio::spawn
//! |-- tokio::sync::oneshot::channel
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/effect-drizzle-sqlite/test/sqlite.test.ts transaction cases.
//! Contract tests: storage-sqlite/tests/transactions.rs

use std::time::Duration;

use opencode_storage_api::{
    Access, Behavior, Database, DatabaseExt, SqlExt, StorageError, TxOptions, Value, WorkOutcome,
};
use opencode_storage_sqlite::{OpenOptions, SqliteDatabase};
use tokio_util::sync::CancellationToken;

async fn bare(path: &std::path::Path) -> SqliteDatabase {
    let options = OpenOptions {
        migrate: false,
        ..OpenOptions::default()
    };
    let db = SqliteDatabase::open_with(path, options).await.unwrap();
    db.transaction(TxOptions::default(), |tx| {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)")
    })
    .await
    .unwrap();
    db
}

async fn names(db: &SqliteDatabase) -> Vec<String> {
    db.read(|tx| {
        tx.query("SELECT name FROM users ORDER BY id", &[])?
            .iter()
            .map(|row| row.get::<String>("name"))
            .collect::<Result<Vec<_>, StorageError>>()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn commits_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    db.transaction(TxOptions::immediate(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES (?1)", &["Grace".into()])
    })
    .await
    .unwrap();
    #[derive(Debug)]
    enum Boom {
        Boom,
        #[allow(dead_code)]
        Storage(StorageError),
    }
    impl From<StorageError> for Boom {
        fn from(e: StorageError) -> Self {
            Boom::Storage(e)
        }
    }
    let result: Result<(), Boom> = db
        .transaction(TxOptions::default(), |tx| {
            tx.execute("INSERT INTO users (name) VALUES (?1)", &["Linus".into()])?;
            Err(Boom::Boom)
        })
        .await;
    assert!(matches!(result, Err(Boom::Boom)));
    assert!(!matches!(result, Err(Boom::Storage(_))));
    assert_eq!(names(&db).await, ["Grace"]);
}

#[tokio::test]
async fn nested_savepoints_roll_back_only_inner() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    db.transaction(TxOptions::default(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('outer')", &[])?;
        let inner: Result<(), StorageError> = tx.savepoint(|sp| {
            sp.execute("INSERT INTO users (name) VALUES ('inner')", &[])?;
            let deeper: Result<(), StorageError> = sp.savepoint(|d| {
                d.execute("INSERT INTO users (name) VALUES ('deeper')", &[])?;
                Err(StorageError::Validation("nope".into()))
            });
            assert!(deeper.is_err());
            Ok(())
        });
        inner?;
        let failed: Result<(), StorageError> = tx.savepoint(|sp| {
            sp.execute("INSERT INTO users (name) VALUES ('dropped')", &[])?;
            sp.execute("INSERT INTO users (id, name) VALUES (1, 'dup')", &[])
                .map(|_| ())
        });
        assert!(matches!(failed, Err(StorageError::UniqueViolation(_))));
        Ok::<_, StorageError>(())
    })
    .await
    .unwrap();
    assert_eq!(names(&db).await, ["outer", "inner"]);
}

#[tokio::test]
async fn panic_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let result: Result<(), StorageError> = db
        .transaction(TxOptions::default(), |tx| {
            tx.execute("INSERT INTO users (name) VALUES ('x')", &[])?;
            panic!("kaboom");
        })
        .await;
    assert!(matches!(result, Err(StorageError::Panicked(m)) if m == "kaboom"));
    assert!(names(&db).await.is_empty());
    // The worker survives a panic.
    db.transaction(TxOptions::default(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('ok')", &[])
    })
    .await
    .unwrap();
    assert_eq!(names(&db).await, ["ok"]);
}

#[tokio::test]
async fn read_access_rejects_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let result = db
        .read(|tx| tx.execute("INSERT INTO users (name) VALUES ('x')", &[]))
        .await;
    assert!(result.is_err(), "{result:?}");
    // query_only is reset afterwards.
    db.transaction(TxOptions::default(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('y')", &[])
    })
    .await
    .unwrap();
    assert_eq!(names(&db).await, ["y"]);
}

#[tokio::test]
async fn failed_commit_of_deferred_foreign_key_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    db.transaction(TxOptions::default(), |tx| {
        tx.execute_batch(
            "CREATE TABLE parent (id TEXT PRIMARY KEY);
             CREATE TABLE child (id TEXT PRIMARY KEY, parent_id TEXT REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);",
        )
    })
    .await
    .unwrap();
    let result = db
        .transaction(TxOptions::default(), |tx| {
            tx.execute(
                "INSERT INTO child (id, parent_id) VALUES ('c', 'missing')",
                &[],
            )?;
            tx.execute("INSERT INTO users (name) VALUES ('lost')", &[])
        })
        .await;
    assert!(
        matches!(result, Err(StorageError::ForeignKey(_))),
        "{result:?}"
    );
    assert!(names(&db).await.is_empty());
    let count: i64 = db
        .read(|tx| {
            tx.query_row("SELECT count(*) AS n FROM child", &[])?
                .unwrap()
                .get::<i64>("n")
        })
        .await
        .unwrap();
    assert_eq!(count, 0);
    // The connection is usable (no transaction left open).
    db.transaction(TxOptions::default(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('after')", &[])
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn immediate_lock_contention_reports_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let a = bare(&path).await;
    let options = OpenOptions {
        migrate: false,
        busy_timeout_ms: Some(50),
        ..OpenOptions::default()
    };
    let b = SqliteDatabase::open_with(&path, options).await.unwrap();
    let (hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn({
        let a = a.clone();
        async move {
            a.transaction(TxOptions::immediate(), move |tx| {
                tx.execute("INSERT INTO users (name) VALUES ('a')", &[])?;
                locked_tx.send(()).unwrap();
                hold_rx.recv_timeout(Duration::from_secs(5)).ok();
                Ok::<_, StorageError>(())
            })
            .await
        }
    });
    locked_rx.await.unwrap();
    let result = b
        .transaction(TxOptions::immediate(), |tx| {
            tx.execute("INSERT INTO users (name) VALUES ('b')", &[])
        })
        .await;
    assert!(matches!(&result, Err(e) if e.is_retryable()), "{result:?}");
    hold_tx.send(()).unwrap();
    holder.await.unwrap().unwrap();
    b.transaction(TxOptions::immediate(), |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('b')", &[])
    })
    .await
    .unwrap();
    assert_eq!(names(&a).await, ["a", "b"]);
}

#[tokio::test]
async fn cancel_before_begin_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let token = CancellationToken::new();
    token.cancel();
    let result = db
        .transaction(TxOptions::default().with_cancel(token), |tx| {
            tx.execute("INSERT INTO users (name) VALUES ('x')", &[])
        })
        .await;
    assert!(matches!(result, Err(StorageError::Cancelled)));
    assert!(names(&db).await.is_empty());
}

#[tokio::test]
async fn cancel_interrupts_running_statement_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    let result = db
        .transaction(TxOptions::default().with_cancel(token), |tx| {
            tx.execute("INSERT INTO users (name) VALUES ('partial')", &[])?;
            // Unbounded recursive CTE: only an interrupt stops it.
            tx.query("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT max(x) FROM c", &[])
                .map(|_| ())
        })
        .await;
    assert!(matches!(result, Err(StorageError::Cancelled)), "{result:?}");
    assert!(names(&db).await.is_empty());
}

#[tokio::test]
async fn completed_work_is_not_cancelled_after_ok() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let token = CancellationToken::new();
    let inner = token.clone();
    db.transaction(TxOptions::default().with_cancel(token), move |tx| {
        tx.execute("INSERT INTO users (name) VALUES ('kept')", &[])?;
        // Cancellation requested after the last statement: the commit still happens.
        inner.cancel();
        Ok::<_, StorageError>(())
    })
    .await
    .unwrap();
    assert_eq!(names(&db).await, ["kept"]);
}

/// The WAL-reset corruption bug (two connections writing/checkpointing at once) is fixed in
/// SQLite 3.51.3 (https://www.sqlite.org/wal.html#the_wal_reset_bug). Guard against a downgrade.
#[test]
fn bundled_sqlite_has_wal_reset_fix() {
    let version: Vec<u32> = opencode_storage_sqlite::sqlite_version()
        .split('.')
        .map(|part| part.parse().unwrap())
        .collect();
    assert!(
        version >= vec![3, 51, 3],
        "bundled SQLite {version:?} predates the WAL-reset fix"
    );
}

/// `PRAGMA journal_mode = WAL` returns SQLITE_BUSY without calling the busy handler when two
/// connections switch a fresh file at once. Open retries it, so both opens succeed.
#[tokio::test]
async fn concurrent_open_of_one_path_migrates_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.sqlite");
    let (a, b) = tokio::join!(SqliteDatabase::open(&path), SqliteDatabase::open(&path));
    let (a, b) = (a.unwrap(), b.unwrap());
    let count = |db: SqliteDatabase| async move {
        db.read(|tx| {
            tx.query_row("SELECT count(*) AS n FROM migration", &[])?
                .unwrap()
                .get::<i64>("n")
        })
        .await
    };
    assert_eq!(count(a).await.unwrap(), 38);
    assert_eq!(count(b).await.unwrap(), 38);
}

#[tokio::test]
async fn read_only_file_rejects_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let db = bare(&path).await;
    db.close().await.unwrap();
    let options = OpenOptions {
        read_only: true,
        migrate: false,
        ..OpenOptions::default()
    };
    let ro = SqliteDatabase::open_with(&path, options).await.unwrap();
    let result = ro
        .transaction(TxOptions::default(), |tx| {
            tx.execute("INSERT INTO users (name) VALUES ('x')", &[])
        })
        .await;
    assert!(
        matches!(result, Err(StorageError::PermissionDenied(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn close_drains_then_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let db = bare(&dir.path().join("a.db")).await;
    let clone = db.clone();
    db.close().await.unwrap();
    let result = clone
        .transaction(TxOptions::default(), |tx| tx.execute("SELECT 1", &[]))
        .await;
    assert!(matches!(result, Err(StorageError::Closed)));
    let raw = clone
        .run(
            Access::Read { cancel: None },
            Box::new(|_| WorkOutcome::Commit(Box::new(()))),
        )
        .await;
    assert!(matches!(raw, Err(StorageError::Closed)));
}

#[tokio::test]
async fn typed_values_round_trip() {
    let db = SqliteDatabase::open_with(
        ":memory:",
        OpenOptions {
            migrate: false,
            ..OpenOptions::default()
        },
    )
    .await
    .unwrap();
    let row = db
        .transaction(
            TxOptions {
                behavior: Behavior::Exclusive,
                cancel: None,
            },
            |tx| {
                tx.query_row(
                    "SELECT ?1 AS i, ?2 AS r, ?3 AS t, ?4 AS b, ?5 AS n",
                    &[
                        Value::Integer(i64::MAX),
                        Value::Real(1.25),
                        "x".into(),
                        Value::Blob(vec![0, 1]),
                        Value::Null,
                    ],
                )
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get::<i64>("i").unwrap(), i64::MAX);
    assert_eq!(row.get::<f64>("r").unwrap(), 1.25);
    assert_eq!(row.get::<Vec<u8>>("b").unwrap(), vec![0, 1]);
    assert_eq!(row.get::<Option<String>>("n").unwrap(), None);
}
