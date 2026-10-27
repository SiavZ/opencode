//! Purpose: durable issuer state, one-time authorization codes and atomic refresh rotation.
//! Scope: native issuer SQLite database, shared safely between processes.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! External:
//! |-- rusqlite::Connection
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! Invariants: taking a code is atomic; refresh reuse returns the same reserved successor.
//! Contract tests: issuer_runtime/tests.rs

use std::{path::Path, sync::Mutex, time::Duration};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{Error, Grant};

/// Required storage operations. Implementations must atomically consume and rotate records.
pub trait IssuerStore: Send + Sync {
    fn put(&self, key: &str, value: &Value, expires: i64) -> Result<(), Error>;
    fn get(&self, key: &str, now: i64) -> Result<Option<Value>, Error>;
    fn take(&self, key: &str, now: i64) -> Result<Option<Value>, Error>;
    fn refresh(&self, key: &str, now: i64) -> Result<Grant, Error>;
}

/// Production native-host adapter. The database path must reside on a persistent volume.
pub struct SqliteStore(Mutex<Connection>);

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(Error::Configuration(
                "persistent issuer store path required",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            if std::fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(Error::Configuration("issuer database may not be a symlink"));
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| Error::Storage)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Storage)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS issuer_record (
               key TEXT PRIMARY KEY, value TEXT NOT NULL, expires INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS issuer_record_expiry ON issuer_record(expires);",
        )?;
        Ok(Self(Mutex::new(connection)))
    }
}

impl IssuerStore for SqliteStore {
    fn put(&self, key: &str, value: &Value, expires: i64) -> Result<(), Error> {
        let connection = self.0.lock().map_err(|_| Error::Storage)?;
        connection.execute(
            "DELETE FROM issuer_record WHERE expires<=?1",
            [super::now()],
        )?;
        connection.execute(
            "INSERT INTO issuer_record(key,value,expires) VALUES (?1,?2,?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, expires=excluded.expires",
            params![key, serde_json::to_string(value)?, expires],
        )?;
        Ok(())
    }

    fn get(&self, key: &str, now: i64) -> Result<Option<Value>, Error> {
        let connection = self.0.lock().map_err(|_| Error::Storage)?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM issuer_record WHERE key=?1 AND expires>?2",
                params![key, now],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .transpose()
    }

    fn take(&self, key: &str, now: i64) -> Result<Option<Value>, Error> {
        let connection = self.0.lock().map_err(|_| Error::Storage)?;
        let value: Option<String> = connection.query_row(
            "DELETE FROM issuer_record WHERE key=?1 RETURNING CASE WHEN expires>?2 THEN value END",
            params![key, now], |row| row.get(0),
        ).optional()?.flatten();
        value
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .transpose()
    }

    fn refresh(&self, key: &str, now: i64) -> Result<Grant, Error> {
        let mut connection = self.0.lock().map_err(|_| Error::Storage)?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let value: Option<String> = transaction
            .query_row(
                "SELECT value FROM issuer_record WHERE key=?1 AND expires>?2",
                params![key, now],
                |row| row.get(0),
            )
            .optional()?;
        let mut grant: Grant = serde_json::from_str(&value.ok_or(Error::InvalidGrant)?)?;
        if let Some(used) = grant.used {
            if now > used + 60 {
                return Err(Error::InvalidGrant);
            }
            return Ok(grant);
        }
        grant.used = Some(now);
        transaction.execute(
            "UPDATE issuer_record SET value=?2, expires=?3 WHERE key=?1",
            params![key, serde_json::to_string(&grant)?, now + 60],
        )?;
        let successor = Grant {
            next: super::random()?,
            used: None,
            ..grant.clone()
        };
        transaction.execute(
            "INSERT INTO issuer_record(key,value,expires) VALUES (?1,?2,?3)",
            params![
                super::refresh_key(&grant.subject, &grant.next),
                serde_json::to_string(&successor)?,
                now + super::REFRESH_TTL
            ],
        )?;
        transaction.commit()?;
        Ok(grant)
    }
}
