//! Purpose: typed Session errors shared by every Session operation, mirroring the tagged errors
//! in `packages/core/src/session.ts` and `session/error.ts`.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- session-api/src/ids.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- session-api/src/lib.rs
//! `-- session-api/src/operations.rs
//! Invariants: not-found, conflict, unavailable, decode, cancelled and retryable storage
//! failures stay distinguishable. `_tag` strings match the baseline for wire mapping.
//! Contract tests: session-api/tests/wire.rs

use crate::ids::{MessageID, SessionID};

/// Operations that V2 explicitly reports as unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableOperation {
    Move,
    Shell,
    Skill,
    SwitchAgent,
    Compact,
    Wait,
}

impl UnavailableOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Shell => "shell",
            Self::Skill => "skill",
            Self::SwitchAgent => "switchAgent",
            Self::Compact => "compact",
            Self::Wait => "wait",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {session_id}")]
    NotFound { session_id: SessionID },
    #[error("Message not found: {message_id}")]
    MessageNotFound {
        session_id: SessionID,
        message_id: MessageID,
    },
    #[error("Prompt {message_id} conflicts with an existing prompt in session {session_id}")]
    PromptConflict {
        session_id: SessionID,
        message_id: MessageID,
    },
    #[error("Session operation unavailable: {}", operation.as_str())]
    OperationUnavailable { operation: UnavailableOperation },
    #[error("Failed to decode message {message_id} in session {session_id}")]
    MessageDecode {
        session_id: SessionID,
        message_id: MessageID,
    },
    #[error("Failed to decode context snapshot for session {session_id}: {details}")]
    ContextSnapshotDecode {
        session_id: SessionID,
        details: String,
    },
    #[error("system context initialization blocked: {0}")]
    ContextInitializationBlocked(String),
    #[error("invalid input: {0}")]
    Validation(String),
    #[error("operation cancelled")]
    Cancelled,
    /// Model, tool or provider failure surfaced by a run. `retryable` follows the source error.
    #[error("run failed: {message}")]
    Run { message: String, retryable: bool },
    /// Storage or event infrastructure failure.
    #[error("storage failure: {message}")]
    Storage { message: String, retryable: bool },
}

impl SessionError {
    /// Baseline `_tag` used by wire error mapping.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "Session.NotFoundError",
            Self::MessageNotFound { .. } => "Session.MessageNotFoundError",
            Self::PromptConflict { .. } => "Session.PromptConflictError",
            Self::OperationUnavailable { .. } => "Session.OperationUnavailableError",
            Self::MessageDecode { .. } => "Session.MessageDecodeError",
            Self::ContextSnapshotDecode { .. } => "Session.ContextSnapshotDecodeError",
            Self::ContextInitializationBlocked(_) => "SystemContext.InitializationBlocked",
            Self::Validation(_) => "Session.ValidationError",
            Self::Cancelled => "Session.Cancelled",
            Self::Run { .. } => "Session.RunError",
            Self::Storage { .. } => "Session.StorageError",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Run {
                retryable: true,
                ..
            } | Self::Storage {
                retryable: true,
                ..
            }
        )
    }
}

pub type SessionResult<T> = Result<T, SessionError>;
