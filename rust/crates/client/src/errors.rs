//! Purpose: separate transport, protocol-shape and declared wire failures, mirroring
//! `ClientError` reasons of the generated Promise client.
//! Scope: error values only.
//! Uses:
//! `-- none
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- client/src/lib.rs
//! Invariants: a declared error status yields `ApiError::Declared` with the decoded wire
//! body, never a `ClientError`. Reasons match packages/client/src/generated/client-error.ts.
//! Contract tests: client/tests (Phase 08)

use thiserror::Error;

/// Reasons of `ClientError` in `packages/client/src/generated/client-error.ts:1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientErrorReason {
    Transport,
    UnexpectedStatus,
    UnsupportedContentType,
    MalformedResponse,
}

impl ClientErrorReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "Transport",
            Self::UnexpectedStatus => "UnexpectedStatus",
            Self::UnsupportedContentType => "UnsupportedContentType",
            Self::MalformedResponse => "MalformedResponse",
        }
    }
}

/// Client-side failure that is not a declared wire error.
#[derive(Debug, Error)]
#[error("{}", reason.as_str())]
pub struct ClientError {
    pub reason: ClientErrorReason,
    /// HTTP status for `UnexpectedStatus`.
    pub status: Option<u16>,
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl ClientError {
    pub fn new(reason: ClientErrorReason) -> Self {
        Self {
            reason,
            status: None,
            cause: None,
        }
    }

    pub fn with_cause(
        reason: ClientErrorReason,
        cause: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        Self {
            reason,
            status: None,
            cause: Some(cause.into()),
        }
    }

    pub fn unexpected_status(status: u16) -> Self {
        Self {
            reason: ClientErrorReason::UnexpectedStatus,
            status: Some(status),
            cause: None,
        }
    }
}

/// Result of a typed operation: declared wire errors stay separate from client failures.
#[derive(Debug, Error)]
pub enum ApiError<E> {
    #[error("declared error status {status}")]
    Declared { status: u16, body: E },
    #[error(transparent)]
    Client(#[from] ClientError),
}
