//! Purpose: SQL executor and async database handle contracts. One transaction context is shared by
//! event commit, projectors and commit hooks so they commit or roll back together.
//! Scope: object-safe traits plus typed generic extension helpers. No engine code.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! `-- storage-api/src/value.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- storage-api/src/lib.rs
//! Invariants: work closures run on the storage worker, never across an await. A write transaction is
//! committed only when the work returns Ok. Commit failure rolls back and reports the commit error.
//! Nested `savepoint` calls map to SQLite SAVEPOINT/RELEASE/ROLLBACK TO, matching
//! packages/effect-drizzle-sqlite/src/effect-sqlite/session.ts:118-187.
//! Contract tests: storage-api/src/sql.rs, storage-sqlite/tests/transactions.rs

use std::any::Any;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::{BoxError, Result, StorageError};
use crate::value::{Row, Value};

/// Synchronous SQL executor valid for the duration of one unit of work.
pub trait Sql {
    /// Execute one statement, returning the number of changed rows.
    fn execute(&self, sql: &str, params: &[Value]) -> Result<usize>;
    /// Execute several `;`-separated statements without parameters.
    fn execute_batch(&self, sql: &str) -> Result<()>;
    /// Run a query and collect every row.
    fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>>;
    /// Run a query and return the first row, if any.
    fn query_row(&self, sql: &str, params: &[Value]) -> Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }
    /// Rowid of the last successful INSERT on this connection.
    fn last_insert_rowid(&self) -> i64;
    /// Object-safe savepoint primitive. Prefer [`SqlExt::savepoint`].
    /// Returns `Ok(true)` when released, `Ok(false)` when rolled back because `work` returned false.
    fn savepoint_dyn(&self, work: &mut dyn FnMut(&dyn Sql) -> bool) -> Result<bool>;
}

/// Typed helpers over [`Sql`].
pub trait SqlExt: Sql {
    /// Run `work` inside a nested savepoint. Err rolls back to the savepoint and is returned unchanged.
    fn savepoint<T, E, F>(&self, work: F) -> std::result::Result<T, E>
    where
        F: FnOnce(&dyn Sql) -> std::result::Result<T, E>,
        E: From<StorageError>,
    {
        let mut work = Some(work);
        let mut outcome = None;
        let released = self.savepoint_dyn(&mut |sql| {
            let result = (work.take().expect("savepoint work runs once"))(sql);
            let ok = result.is_ok();
            outcome = Some(result);
            ok
        });
        match (released, outcome) {
            (Err(error), Some(Ok(_)) | None) => Err(E::from(error)),
            (_, Some(result)) => result,
            (Ok(_), None) => Err(E::from(StorageError::Other(
                "savepoint work did not run".into(),
            ))),
        }
    }
}

impl<S: Sql + ?Sized> SqlExt for S {}

/// SQLite BEGIN behavior for write transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Behavior {
    #[default]
    Deferred,
    Immediate,
    Exclusive,
}

/// Options for one unit of work.
#[derive(Debug, Clone, Default)]
pub struct TxOptions {
    pub behavior: Behavior,
    /// Cancelling before BEGIN returns `Cancelled` without touching the database. Cancelling while
    /// a statement runs interrupts it, rolls back, and returns `Cancelled`. Once `work` returned Ok
    /// the commit is not interrupted, so an acknowledged result is always durable.
    pub cancel: Option<CancellationToken>,
}

impl TxOptions {
    pub fn immediate() -> Self {
        Self {
            behavior: Behavior::Immediate,
            cancel: None,
        }
    }

    pub fn with_cancel(mut self, token: CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }
}

/// Kind of unit of work.
#[derive(Debug, Clone)]
pub enum Access {
    /// Read-only snapshot. Writes are rejected by the engine.
    Read { cancel: Option<CancellationToken> },
    /// Write transaction. Committed when the work commits.
    Write(TxOptions),
}

/// Result of the type-erased work closure.
pub enum WorkOutcome {
    /// Commit (write) or finish (read) and return the value.
    Commit(Box<dyn Any + Send>),
    /// Roll back and return the value (usually the caller's error).
    Rollback(Box<dyn Any + Send>),
}

impl std::fmt::Debug for WorkOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkOutcome::Commit(_) => f.write_str("Commit(..)"),
            WorkOutcome::Rollback(_) => f.write_str("Rollback(..)"),
        }
    }
}

/// Type-erased work run on the storage worker.
pub type Work = Box<dyn FnOnce(&dyn Sql) -> WorkOutcome + Send + 'static>;

/// Process-global SQL database handle. Implemented by `opencode-storage-sqlite`.
#[async_trait]
pub trait Database: Send + Sync + 'static {
    /// Run one unit of work. `Err` means BEGIN, COMMIT, cancellation or the worker failed;
    /// a failed COMMIT has already been rolled back.
    async fn run(&self, access: Access, work: Work) -> Result<WorkOutcome>;
    /// Close the handle. Pending work completes first, later calls fail with `Closed`.
    async fn close(&self) -> Result<()>;
}

/// Typed helpers over [`Database`].
#[async_trait]
pub trait DatabaseExt: Database {
    /// Run `work` in a write transaction. Ok commits, Err rolls back.
    async fn transaction<T, E, F>(&self, options: TxOptions, work: F) -> std::result::Result<T, E>
    where
        F: FnOnce(&dyn Sql) -> std::result::Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<StorageError> + Send + 'static,
    {
        finish(self.run(Access::Write(options), erase(work)).await)
    }

    /// Run `work` against a read-only snapshot.
    async fn read<T, E, F>(&self, work: F) -> std::result::Result<T, E>
    where
        F: FnOnce(&dyn Sql) -> std::result::Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<StorageError> + Send + 'static,
    {
        finish(self.run(Access::Read { cancel: None }, erase(work)).await)
    }
}

impl<D: Database + ?Sized> DatabaseExt for D {}

fn erase<T, E, F>(work: F) -> Work
where
    F: FnOnce(&dyn Sql) -> std::result::Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    Box::new(move |sql| match work(sql) {
        Ok(value) => WorkOutcome::Commit(Box::new(value)),
        Err(error) => WorkOutcome::Rollback(Box::new(error)),
    })
}

fn finish<T: 'static, E: From<StorageError> + 'static>(
    outcome: Result<WorkOutcome>,
) -> std::result::Result<T, E> {
    match outcome? {
        WorkOutcome::Commit(value) => value
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| E::from(StorageError::Other("work outcome type mismatch".into()))),
        WorkOutcome::Rollback(error) => Err(error
            .downcast::<E>()
            .map(|error| *error)
            .unwrap_or_else(|_| E::from(StorageError::Other("work error type mismatch".into())))),
    }
}

/// Convert any error into a boxed error. Convenience for projector and hook boundaries.
pub fn boxed<E: std::error::Error + Send + Sync + 'static>(error: E) -> BoxError {
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Arc;

    /// In-memory executor that records statements. Test fixture only.
    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);

    impl Sql for Recorder {
        fn execute(&self, sql: &str, _: &[Value]) -> Result<usize> {
            self.0.borrow_mut().push(sql.to_owned());
            if sql == "fail" {
                return Err(StorageError::UniqueViolation("x".into()));
            }
            Ok(1)
        }
        fn execute_batch(&self, sql: &str) -> Result<()> {
            self.execute(sql, &[]).map(|_| ())
        }
        fn query(&self, _: &str, _: &[Value]) -> Result<Vec<Row>> {
            Ok(vec![Row::new(
                Arc::from(vec!["a".to_owned()]),
                vec![Value::Integer(1)],
            )])
        }
        fn last_insert_rowid(&self) -> i64 {
            0
        }
        fn savepoint_dyn(&self, work: &mut dyn FnMut(&dyn Sql) -> bool) -> Result<bool> {
            self.0.borrow_mut().push("savepoint".into());
            let ok = work(self);
            self.0
                .borrow_mut()
                .push(if ok { "release" } else { "rollback to" }.into());
            Ok(ok)
        }
    }

    #[test]
    fn savepoint_propagates_value_and_error() {
        let sql = Recorder::default();
        let value: Result<i32> = sql.savepoint(|tx| tx.execute("a", &[]).map(|_| 7));
        assert_eq!(value.unwrap(), 7);
        let error: Result<i32> = sql.savepoint(|tx| tx.execute("fail", &[]).map(|_| 7));
        assert!(matches!(error, Err(StorageError::UniqueViolation(_))));
        assert_eq!(
            *sql.0.borrow(),
            [
                "savepoint",
                "a",
                "release",
                "savepoint",
                "fail",
                "rollback to"
            ]
        );
        assert_eq!(
            sql.query_row("", &[])
                .unwrap()
                .unwrap()
                .get::<i64>("a")
                .unwrap(),
            1
        );
    }
}
