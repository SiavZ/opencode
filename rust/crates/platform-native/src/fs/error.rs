//! Purpose: map `std::io::Error` into `PlatformError` with the same reason split as the baseline
//! node adapter (`@effect/platform-node-shared/src/internal/utils.ts` `handleErrnoException`).
//! Scope: error conversion only.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! External:
//! |-- nix::errno::Errno::EPERM
//! `-- tokio::task::JoinError
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/glob.rs
//! |-- platform-native/src/fs/mod.rs
//! |-- platform-native/src/fs/mutation.rs
//! |-- platform-native/src/fs/native.rs
//! `-- platform-native/src/fs/watcher.rs
//! Invariants: ENOENT is NotFound, EACCES is PermissionDenied, EEXIST is AlreadyExists, EISDIR and
//! ENOTDIR keep their own kinds, EPERM and everything else are Io (baseline reason "Unknown").
//! Contract tests: platform-native/tests/fs_util.rs

use opencode_platform_api::{PlatformError, PlatformErrorKind};
use std::io::ErrorKind;
use std::path::Path;

pub(crate) fn io_error(operation: &str, path: &Path, error: &std::io::Error) -> PlatformError {
    PlatformError::new(
        kind(error),
        operation,
        path.to_string_lossy(),
        error.to_string(),
    )
}

fn kind(error: &std::io::Error) -> PlatformErrorKind {
    match error.kind() {
        ErrorKind::NotFound => PlatformErrorKind::NotFound,
        ErrorKind::PermissionDenied if is_eperm(error) => PlatformErrorKind::Io,
        ErrorKind::PermissionDenied => PlatformErrorKind::PermissionDenied,
        ErrorKind::AlreadyExists => PlatformErrorKind::AlreadyExists,
        ErrorKind::NotADirectory => PlatformErrorKind::NotADirectory,
        ErrorKind::IsADirectory => PlatformErrorKind::IsADirectory,
        ErrorKind::InvalidInput | ErrorKind::InvalidData => PlatformErrorKind::InvalidInput,
        ErrorKind::TimedOut => PlatformErrorKind::TimedOut,
        _ => PlatformErrorKind::Io,
    }
}

#[cfg(unix)]
fn is_eperm(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(nix::errno::Errno::EPERM as i32)
}

#[cfg(not(unix))]
fn is_eperm(_: &std::io::Error) -> bool {
    false
}

pub(crate) fn join_error(
    operation: &str,
    path: &Path,
    error: tokio::task::JoinError,
) -> PlatformError {
    let kind = if error.is_cancelled() {
        PlatformErrorKind::Cancelled
    } else {
        PlatformErrorKind::Io
    };
    PlatformError::new(kind, operation, path.to_string_lossy(), error.to_string())
}
