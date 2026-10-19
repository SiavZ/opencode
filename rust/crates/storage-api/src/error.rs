//! Purpose: typed storage errors that keep retryable, cancelled, conflict, validation and not-found distinct.
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- storage-api/src/json_store.rs
//! |-- storage-api/src/lib.rs
//! |-- storage-api/src/records/codec.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-api/src/records/mod.rs
//! |-- storage-api/src/sql.rs
//! `-- storage-api/src/value.rs
//! Invariants: `Busy` and `Locked` are the only retryable variants. Constraint failures are never retryable.
//! Contract tests: storage-api/src/error.rs

/// Boxed error used to carry caller-defined failures through object-safe boundaries.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Failure of a storage operation.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// SQLITE_BUSY after the busy timeout elapsed. Retryable.
    #[error("database is busy: {0}")]
    Busy(String),
    /// SQLITE_LOCKED (shared-cache or same-connection table lock). Retryable.
    #[error("database table is locked: {0}")]
    Locked(String),
    /// A UNIQUE or PRIMARY KEY constraint rejected the write.
    #[error("unique constraint failed: {0}")]
    UniqueViolation(String),
    /// A FOREIGN KEY constraint rejected the write.
    #[error("foreign key constraint failed: {0}")]
    ForeignKey(String),
    /// NOT NULL, CHECK or other constraint failure.
    #[error("constraint failed: {0}")]
    Constraint(String),
    /// A requested record or file does not exist.
    #[error("Resource not found: {0}")]
    NotFound(String),
    /// The operation was cancelled before commit. Nothing was written.
    #[error("storage operation cancelled")]
    Cancelled,
    /// A stored value could not be decoded into the requested type.
    #[error("decode failed for {column}: {message}")]
    Decode { column: String, message: String },
    /// Input rejected before reaching the database (for example a relative path).
    #[error("invalid input: {0}")]
    Validation(String),
    /// The database is read-only or the file system denied access.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// The disk or database file is full.
    #[error("storage full: {0}")]
    Full(String),
    /// The database file is corrupt or not a database.
    #[error("database corrupt: {0}")]
    Corrupt(String),
    /// Schema state that the migrator refuses to touch.
    #[error("migration failed: {0}")]
    Migration(String),
    /// The handle was closed.
    #[error("database is closed")]
    Closed,
    /// Work panicked inside the storage worker. The transaction was rolled back.
    #[error("storage worker panicked: {0}")]
    Panicked(String),
    /// I/O failure outside SQLite (legacy JSON store, directories).
    #[error("io error: {0}")]
    Io(String),
    /// Any other engine failure.
    #[error("storage error: {0}")]
    Other(String),
}

impl StorageError {
    /// True when retrying the same operation later may succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, StorageError::Busy(_) | StorageError::Locked(_))
    }

    /// True for any constraint rejection (unique, foreign key, not null, check).
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            StorageError::UniqueViolation(_)
                | StorageError::ForeignKey(_)
                | StorageError::Constraint(_)
        )
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, StorageError::NotFound(_))
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, StorageError::Cancelled)
    }
}

pub type Result<T, E = StorageError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(StorageError::Busy("x".into()).is_retryable());
        assert!(!StorageError::UniqueViolation("x".into()).is_retryable());
        assert!(StorageError::ForeignKey("x".into()).is_conflict());
        assert!(StorageError::NotFound("a".into()).is_not_found());
        assert_eq!(
            StorageError::NotFound("/x/y.json".into()).to_string(),
            "Resource not found: /x/y.json"
        );
    }
}
