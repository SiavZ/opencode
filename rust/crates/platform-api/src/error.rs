//! Purpose: one platform error type preserving not-found/exists/permission/cancel/timeout distinctions.
//! Scope: error values only.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/lib.rs
//! |-- platform-api/src/process.rs
//! |-- platform-api/src/pty.rs
//! |-- platform-api/src/watcher.rs
//! |-- platform-native/src/process/mod.rs
//! `-- session-runtime/tests/support/legacy.rs
//! Invariants: kind is stable and matchable, message is human text for logs and tool output.
//! Contract tests: none

use serde::{Deserialize, Serialize};

/// Classification mirroring effect PlatformError reasons used by the baseline (`NotFound`,
/// `AlreadyExists`, ...), plus cancellation and timeout distinctions required by tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformErrorKind {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    NotADirectory,
    IsADirectory,
    InvalidInput,
    Cancelled,
    TimedOut,
    Unsupported,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation} {path}: {message}")]
pub struct PlatformError {
    pub kind: PlatformErrorKind,
    pub operation: String,
    pub path: String,
    pub message: String,
}

impl PlatformError {
    pub fn new(
        kind: PlatformErrorKind,
        operation: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation: operation.into(),
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn is_not_found(&self) -> bool {
        self.kind == PlatformErrorKind::NotFound
    }

    pub fn unsupported(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(PlatformErrorKind::Unsupported, operation, "", message)
    }
}
