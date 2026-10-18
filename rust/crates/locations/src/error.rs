//! Purpose: typed failures of the locations crate (registry, project identity, workspace lifecycle).
//! Scope: error enum plus storage/event conversions.
//! Uses:
//! `-- none
//! External:
//! |-- opencode_event_api::EventError
//! `-- opencode_storage_api::StorageError
//! Used by:
//! rust/crates/
//! |-- locations/src/git.rs
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/project.rs
//! |-- locations/src/project_store.rs
//! |-- locations/src/registry.rs
//! |-- locations/src/workspace/adapter.rs
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! `-- locations/src/workspace/worktree.rs
//! Invariants: retryable/cancelled/conflict/validation/not-found distinctions of the storage layer
//! survive conversion. Messages that reach the wire match the baseline text.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

use opencode_event_api::EventError;
use opencode_storage_api::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum LocationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Event(#[from] EventError),
    /// The per-location bundle builder failed. Nothing was cached for the Ref.
    #[error("location services failed to boot: {message}")]
    Build { message: String },
    /// The registry was shut down.
    #[error("location registry is closed")]
    Closed,
    /// Input rejected before any side effect (wrong ID prefix, malformed adapter config).
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    NotFound(String),
    /// `Worktree.NotGitError`.
    #[error("Worktrees are only supported for git projects")]
    NotGit,
    /// A git command failed. `message` is stderr, stdout or the baseline fallback text.
    #[error("{message}")]
    Git { message: String },
    #[error("io error: {0}")]
    Io(String),
    /// A workspace adapter failed (`WorkspaceAdapterRuntime` rejections).
    #[error("{message}")]
    Adapter { message: String },
    /// `Workspace.create` persisted the row but the adapter `create` failed. The row is left in
    /// place like the baseline. Call `WorkspaceService::remove(workspace_id)` to clean up.
    #[error("{source}")]
    CreateFailed {
        workspace_id: String,
        #[source]
        source: Box<LocationError>,
    },
    /// `waitEvent` timeout.
    #[error("Timed out waiting for global event")]
    Timeout,
    /// `WorkspaceSyncHttpError`.
    #[error("{message}")]
    SyncHttp {
        message: String,
        status: u16,
        body: Option<String>,
    },
    /// `WorkspaceSyncTimeoutError`: `Timed out waiting for sync fence: <state json>`.
    #[error("{message}")]
    SyncTimeout { message: String },
    /// `WorkspaceSyncAbortedError`.
    #[error("{message}")]
    SyncAborted { message: String },
    #[error("operation cancelled")]
    Cancelled,
    /// Capability owned by a later phase (auth, session removal, remote transport).
    #[error("{operation} is not yet ported: {message}")]
    Unsupported { operation: String, message: String },
}

impl LocationError {
    pub fn unsupported(operation: impl Into<String>, message: impl Into<String>) -> Self {
        LocationError::Unsupported {
            operation: operation.into(),
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            LocationError::Storage(error) => error.is_retryable(),
            LocationError::Event(error) => error.is_retryable(),
            _ => false,
        }
    }

    pub fn is_not_found(&self) -> bool {
        match self {
            LocationError::NotFound(_) => true,
            LocationError::Storage(error) => error.is_not_found(),
            _ => false,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        match self {
            LocationError::Cancelled => true,
            LocationError::Storage(error) => error.is_cancelled(),
            LocationError::Event(EventError::Cancelled) => true,
            _ => false,
        }
    }
}

impl From<std::io::Error> for LocationError {
    fn from(error: std::io::Error) -> Self {
        LocationError::Io(error.to_string())
    }
}

pub type Result<T, E = LocationError> = std::result::Result<T, E>;
