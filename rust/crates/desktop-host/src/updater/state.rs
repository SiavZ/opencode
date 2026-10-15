//! Purpose: wire types for the updater: the renderer-visible `UpdaterState` union, the persisted
//! ready record and the typed updater error.
//! Scope: data only, no IO.
//! Uses:
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/updater.rs
//! |-- desktop-host/src/updater/controller.rs
//! |-- desktop-host/src/updater/feed.rs
//! |-- desktop-host/src/updater/signature.rs
//! `-- desktop-host/src/updater/store.rs
//! Invariants: JSON shape matches `packages/app/src/updater.ts` `UpdaterState` exactly
//! (`status` discriminant, kebab-case `up-to-date`, `percent` omitted when absent).
//! Contract tests: desktop-host/tests/updater_controller.rs

use serde::{Deserialize, Serialize};

/// Mirror of `UpdaterState` in `packages/app/src/updater.ts:3-11`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum UpdaterState {
    Disabled,
    Idle,
    Checking,
    Downloading {
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<f64>,
    },
    Ready {
        version: String,
    },
    UpToDate,
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

impl UpdaterState {
    /// The `status` discriminant as the renderer sees it.
    pub fn status(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::Downloading { .. } => "downloading",
            Self::Ready { .. } => "ready",
            Self::UpToDate => "up-to-date",
            Self::Installing { .. } => "installing",
            Self::Error { .. } => "error",
        }
    }
}

/// Persisted under key `ready` in the `opencode.updater` store (`updater.ts:11,51-56`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdaterReadyRecord {
    pub version: String,
}

/// Updater failures. `Display` text becomes `UpdaterState::Error.message`.
#[derive(Debug, thiserror::Error)]
pub enum UpdaterError {
    /// Same text as `updater-controller.ts:81`.
    #[error("Update is not ready to install")]
    NotReady,
    #[error("update feed request failed: {message}")]
    Network { message: String, retryable: bool },
    #[error("update feed is invalid: {0}")]
    InvalidFeed(String),
    #[error("update feed has no entry for platform {0}")]
    MissingPlatform(String),
    #[error("update signature verification failed: {0}")]
    Signature(String),
    #[error("updater persistence failed: {0}")]
    Persistence(String),
    #[error("updater channel is invalid: {0}")]
    InvalidChannel(String),
    #[error("no update has been downloaded")]
    NothingDownloaded,
    #[error("{0}")]
    Stop(String),
    /// Typed gap: a capability the port does not provide yet. Recorded in notes/shell.md.
    #[error("not yet ported: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Backend(String),
}

impl UpdaterError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network {
                retryable: true,
                ..
            }
        )
    }
}
