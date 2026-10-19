//! Purpose: typed context failures and boxed producer futures shared by context services.
//! Scope: source and persistence boundary types, no side effects.
//! Uses:
//! rust/crates/
//! |-- context/src/source/snapshot.rs
//! |-- event-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- context/src/source/mod.rs
//! Invariants: unavailable initialization, duplicate identity, rendering and persistence failures stay distinct.
//! Contract tests: context/tests/system_context.rs, context/tests/registry.rs

use std::future::Future;
use std::pin::Pin;

use super::snapshot::Key;

/// Boxed error from a source producer (the baseline's defect channel).
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;
/// Boxed `Send` future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Errors of the context crate. session-runtime maps them onto `SessionError`.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// `SystemContext.InitializationBlocked`.
    #[error("System context initialization blocked by unavailable sources: {}", join_keys(.keys))]
    InitializationBlocked { keys: Vec<Key> },
    /// `SystemContext.DuplicateKeyError`.
    #[error("Duplicate system context key: {key}")]
    DuplicateKey { key: Key },
    /// Registry entry key reused while the first registration is alive.
    #[error("Duplicate system context entry key: {key}")]
    DuplicateEntry { key: Key },
    /// A source rendered an empty baseline, update or removal.
    #[error("System context source {key} rendered an empty {kind}")]
    EmptyRendering { key: Key, kind: &'static str },
    /// `Session.ContextSnapshotDecodeError`.
    #[error("Failed to decode context snapshot for session {session_id}: {details}")]
    SnapshotDecode { session_id: String, details: String },
    /// The epoch row vanished between read and write.
    #[error("Context Epoch not found")]
    EpochNotFound { session_id: String },
    /// A source or registry entry producer failed.
    #[error("{0}")]
    Source(BoxError),
    #[error(transparent)]
    Storage(#[from] opencode_storage_api::StorageError),
    #[error(transparent)]
    Event(#[from] opencode_event_api::EventError),
}

fn join_keys(keys: &[Key]) -> String {
    keys.iter().map(Key::as_str).collect::<Vec<_>>().join(", ")
}
