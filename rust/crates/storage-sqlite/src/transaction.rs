//! Purpose: `storage_api::Sql` over a rusqlite connection for one unit of work: statements, typed rows,
//! SAVEPOINT nesting, BEGIN/COMMIT/ROLLBACK and the rusqlite -> `StorageError` classification.
//! Scope: runs only on the connection worker thread, valid for one unit of work.
//! Uses:
//! rust/crates/
//! `-- storage-api/src/lib.rs
//! External:
//! |-- ffi::SQLITE_CONSTRAINT_FOREIGNKEY
//! |-- ffi::SQLITE_CONSTRAINT_PRIMARYKEY
//! |-- ffi::SQLITE_CONSTRAINT_UNIQUE
//! |-- rusqlite::Connection
//! |-- rusqlite::Error
//! |-- rusqlite::Error::FromSqlConversionFailure
//! |-- rusqlite::Error::InvalidColumnType
//! |-- rusqlite::Error::InvalidParameterCount
//! |-- rusqlite::Error::InvalidParameterName
//! |-- rusqlite::Error::MultipleStatement
//! |-- rusqlite::Error::QueryReturnedNoRows
//! |-- rusqlite::Error::SqliteFailure
//! |-- rusqlite::ErrorCode
//! |-- rusqlite::ffi
//! |-- rusqlite::params_from_iter
//! |-- rusqlite::types::Value
//! |-- rusqlite::types::ValueRef
//! |-- str::to_owned
//! |-- usize::MAX
//! `-- usize::try_from
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/connection.rs
//! |-- storage-sqlite/src/lib.rs
//! |-- storage-sqlite/src/migration.rs
//! `-- storage-sqlite/src/migrations/mod.rs
//! Invariants: nested savepoints are named `effect_sql_<depth>` starting at 1 and are released on success,
//! rolled back then released on failure (packages/effect-drizzle-sqlite/src/effect-sqlite/session.ts:144-172).
//! Every statement first checks the cancellation flag so cancellation between statements is observed.
//! Contract tests: storage-sqlite/tests/transactions.rs

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use opencode_storage_api::{Behavior, Row, Sql, StorageError, Value};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, ErrorCode, ffi};

/// Executor bound to the worker connection for one unit of work.
pub(crate) struct Tx<'c> {
    conn: &'c Connection,
    depth: Cell<u32>,
    cancelled: Arc<AtomicBool>,
}

impl<'c> Tx<'c> {
    pub(crate) fn new(conn: &'c Connection, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            conn,
            depth: Cell::new(0),
            cancelled,
        }
    }

    fn check(&self) -> Result<(), StorageError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(StorageError::Cancelled);
        }
        Ok(())
    }
}

impl Sql for Tx<'_> {
    fn execute(&self, sql: &str, params: &[Value]) -> Result<usize, StorageError> {
        self.check()?;
        let mut stmt = self.conn.prepare_cached(sql).map_err(map_error)?;
        let params = params.iter().map(to_sql).collect::<Vec<_>>();
        if stmt.column_count() == 0 {
            return stmt
                .execute(rusqlite::params_from_iter(params))
                .map_err(map_error);
        }
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params))
            .map_err(map_error)?;
        while rows.next().map_err(map_error)?.is_some() {}
        Ok(usize::try_from(self.conn.changes()).unwrap_or(usize::MAX))
    }

    fn execute_batch(&self, sql: &str) -> Result<(), StorageError> {
        self.check()?;
        self.conn.execute_batch(sql).map_err(map_error)
    }

    fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>, StorageError> {
        self.check()?;
        query(self.conn, sql, params)
    }

    fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    fn savepoint_dyn(&self, work: &mut dyn FnMut(&dyn Sql) -> bool) -> Result<bool, StorageError> {
        self.check()?;
        let id = self.depth.get() + 1;
        self.conn
            .execute_batch(&format!("SAVEPOINT effect_sql_{id}"))
            .map_err(map_error)?;
        self.depth.set(id);
        let ok = work(self);
        self.depth.set(id - 1);
        if ok {
            self.conn
                .execute_batch(&format!("RELEASE SAVEPOINT effect_sql_{id}"))
                .map_err(map_error)?;
            return Ok(true);
        }
        // An interrupt may already have rolled back the whole transaction.
        if !self.conn.is_autocommit() {
            self.conn
                .execute_batch(&format!(
                    "ROLLBACK TO SAVEPOINT effect_sql_{id}; RELEASE SAVEPOINT effect_sql_{id}"
                ))
                .map_err(map_error)?;
        }
        Ok(false)
    }
}

/// Run a query on a raw connection and collect typed rows.
pub(crate) fn query(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Row>, StorageError> {
    let mut stmt = conn.prepare_cached(sql).map_err(map_error)?;
    let columns: Arc<[String]> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let width = columns.len();
    let params = params.iter().map(to_sql).collect::<Vec<_>>();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params))
        .map_err(map_error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(map_error)? {
        let values = (0..width)
            .map(|index| row.get_ref(index).map(from_sql).map_err(map_error))
            .collect::<Result<Vec<_>, _>>()?;
        out.push(Row::new(columns.clone(), values));
    }
    Ok(out)
}

pub(crate) fn begin(conn: &Connection, behavior: Behavior) -> Result<(), StorageError> {
    let sql = match behavior {
        Behavior::Deferred => "BEGIN DEFERRED",
        Behavior::Immediate => "BEGIN IMMEDIATE",
        Behavior::Exclusive => "BEGIN EXCLUSIVE",
    };
    conn.execute_batch(sql).map_err(map_error)
}

/// COMMIT, and on failure roll back so SQLite never keeps the transaction open
/// (deferred constraint failures leave it open, session.ts:155-162).
pub(crate) fn commit(conn: &Connection) -> Result<(), StorageError> {
    match conn.execute_batch("COMMIT") {
        Ok(()) => Ok(()),
        Err(error) => {
            rollback(conn);
            Err(map_error(error))
        }
    }
}

/// ROLLBACK when a transaction is still open. Errors are ignored, like the baseline's rollback-after-failure.
pub(crate) fn rollback(conn: &Connection) {
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
}

pub(crate) fn to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Integer(v) => SqlValue::Integer(*v),
        Value::Real(v) => SqlValue::Real(*v),
        Value::Text(v) => SqlValue::Text(v.clone()),
        Value::Blob(v) => SqlValue::Blob(v.clone()),
    }
}

pub(crate) fn from_sql(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => Value::Integer(v),
        ValueRef::Real(v) => Value::Real(v),
        ValueRef::Text(v) => Value::Text(String::from_utf8_lossy(v).into_owned()),
        ValueRef::Blob(v) => Value::Blob(v.to_vec()),
    }
}

/// Classify a rusqlite error, preserving retryable/conflict/cancelled/permission distinctions.
pub fn map_error(error: rusqlite::Error) -> StorageError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, message) => {
            let text = message.clone().unwrap_or_else(|| failure.to_string());
            match failure.code {
                ErrorCode::DatabaseBusy => StorageError::Busy(text),
                ErrorCode::DatabaseLocked => StorageError::Locked(text),
                ErrorCode::ConstraintViolation => match failure.extended_code {
                    ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
                        StorageError::UniqueViolation(text)
                    }
                    ffi::SQLITE_CONSTRAINT_FOREIGNKEY => StorageError::ForeignKey(text),
                    _ => StorageError::Constraint(text),
                },
                ErrorCode::OperationInterrupted => StorageError::Cancelled,
                ErrorCode::ReadOnly
                | ErrorCode::PermissionDenied
                | ErrorCode::AuthorizationForStatementDenied => {
                    StorageError::PermissionDenied(text)
                }
                ErrorCode::DiskFull => StorageError::Full(text),
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => StorageError::Corrupt(text),
                ErrorCode::CannotOpen | ErrorCode::SystemIoFailure => StorageError::Io(text),
                _ => StorageError::Other(text),
            }
        }
        rusqlite::Error::QueryReturnedNoRows => StorageError::NotFound(error.to_string()),
        rusqlite::Error::MultipleStatement => {
            StorageError::Validation("multiple statements in one call, use execute_batch".into())
        }
        rusqlite::Error::FromSqlConversionFailure(index, _, source) => StorageError::Decode {
            column: index.to_string(),
            message: source.to_string(),
        },
        rusqlite::Error::InvalidColumnType(index, name, kind) => StorageError::Decode {
            column: format!("{index}:{name}"),
            message: format!("unexpected {kind}"),
        },
        rusqlite::Error::InvalidParameterCount(..) | rusqlite::Error::InvalidParameterName(_) => {
            StorageError::Validation(error.to_string())
        }
        _ => StorageError::Other(error.to_string()),
    }
}
