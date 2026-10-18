//! Purpose: tagged HTTP error envelopes of the current Server API (`packages/protocol/src/errors.ts`)
//! with their status codes and IR definitions. Does not map domain errors (server owns that).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- protocol/src/ir.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! `-- protocol/src/operation.rs
//! Invariants: JSON is `{"_tag": <name>, ...fields}` with fields in baseline declaration order and
//! absent optional fields omitted (never null). Status codes match `httpApiStatus`.
//! Contract tests: protocol/tests/errors.rs

use crate::ir::{self, Definition, Schema};
use serde::{Deserialize, Serialize};

/// Error tag discriminant and HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    InvalidRequest,
    Unauthorized,
    Conflict,
    ServiceUnavailable,
    Unknown,
    ProviderNotFound,
    SessionNotFound,
    MessageNotFound,
    InvalidCursor,
    PermissionNotFound,
    QuestionNotFound,
    Forbidden,
    PtyNotFound,
}

impl ErrorKind {
    pub const ALL: [ErrorKind; 13] = [
        ErrorKind::InvalidRequest,
        ErrorKind::Unauthorized,
        ErrorKind::Conflict,
        ErrorKind::ServiceUnavailable,
        ErrorKind::Unknown,
        ErrorKind::ProviderNotFound,
        ErrorKind::SessionNotFound,
        ErrorKind::MessageNotFound,
        ErrorKind::InvalidCursor,
        ErrorKind::PermissionNotFound,
        ErrorKind::QuestionNotFound,
        ErrorKind::Forbidden,
        ErrorKind::PtyNotFound,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            ErrorKind::InvalidRequest => "InvalidRequestError",
            ErrorKind::Unauthorized => "UnauthorizedError",
            ErrorKind::Conflict => "ConflictError",
            ErrorKind::ServiceUnavailable => "ServiceUnavailableError",
            ErrorKind::Unknown => "UnknownError",
            ErrorKind::ProviderNotFound => "ProviderNotFoundError",
            ErrorKind::SessionNotFound => "SessionNotFoundError",
            ErrorKind::MessageNotFound => "MessageNotFoundError",
            ErrorKind::InvalidCursor => "InvalidCursorError",
            ErrorKind::PermissionNotFound => "PermissionNotFoundError",
            ErrorKind::QuestionNotFound => "QuestionNotFoundError",
            ErrorKind::Forbidden => "ForbiddenError",
            ErrorKind::PtyNotFound => "PtyNotFoundError",
        }
    }

    pub fn status(self) -> u16 {
        match self {
            ErrorKind::InvalidRequest | ErrorKind::InvalidCursor => 400,
            ErrorKind::Unauthorized => 401,
            ErrorKind::Forbidden => 403,
            ErrorKind::ProviderNotFound
            | ErrorKind::SessionNotFound
            | ErrorKind::MessageNotFound
            | ErrorKind::PermissionNotFound
            | ErrorKind::QuestionNotFound
            | ErrorKind::PtyNotFound => 404,
            ErrorKind::Conflict => 409,
            ErrorKind::Unknown => 500,
            ErrorKind::ServiceUnavailable => 503,
        }
    }

    /// IR definition: `_tag` literal then fields in declaration order.
    pub fn definition(self) -> Definition {
        let s = ir::string;
        let fields = match self {
            ErrorKind::InvalidRequest => vec![
                ir::req("message", s()),
                ir::opt("kind", s()),
                ir::opt("field", s()),
            ],
            ErrorKind::Unauthorized | ErrorKind::InvalidCursor | ErrorKind::Forbidden => {
                vec![ir::req("message", s())]
            }
            ErrorKind::Conflict => vec![ir::req("message", s()), ir::opt("resource", s())],
            ErrorKind::ServiceUnavailable => vec![ir::req("message", s()), ir::opt("service", s())],
            ErrorKind::Unknown => vec![ir::req("message", s()), ir::opt("ref", s())],
            ErrorKind::ProviderNotFound => {
                vec![ir::req("providerID", s()), ir::req("message", s())]
            }
            ErrorKind::SessionNotFound => vec![ir::req("sessionID", s()), ir::req("message", s())],
            ErrorKind::MessageNotFound => vec![
                ir::req("sessionID", s()),
                ir::req("messageID", s()),
                ir::req("message", s()),
            ],
            ErrorKind::PermissionNotFound | ErrorKind::QuestionNotFound => {
                vec![ir::req("requestID", s()), ir::req("message", s())]
            }
            ErrorKind::PtyNotFound => vec![ir::req("ptyID", s()), ir::req("message", s())],
        };
        let mut all = vec![ir::req("_tag", ir::lit(self.tag()))];
        all.extend(fields);
        ir::def(
            self.tag(),
            Schema::Struct {
                fields: all,
                index: None,
            },
        )
    }
}

/// A current-API error value. Serializes to the exact baseline JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "_tag")]
pub enum ApiError {
    #[error("{message}")]
    InvalidRequestError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        field: Option<String>,
    },
    #[error("{message}")]
    UnauthorizedError { message: String },
    #[error("{message}")]
    ConflictError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        resource: Option<String>,
    },
    #[error("{message}")]
    ServiceUnavailableError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        service: Option<String>,
    },
    #[error("{message}")]
    UnknownError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default, rename = "ref")]
        reference: Option<String>,
    },
    #[error("{message}")]
    ProviderNotFoundError {
        #[serde(rename = "providerID")]
        provider_id: String,
        message: String,
    },
    #[error("{message}")]
    SessionNotFoundError {
        #[serde(rename = "sessionID")]
        session_id: String,
        message: String,
    },
    #[error("{message}")]
    MessageNotFoundError {
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        message: String,
    },
    #[error("{message}")]
    InvalidCursorError { message: String },
    #[error("{message}")]
    PermissionNotFoundError {
        #[serde(rename = "requestID")]
        request_id: String,
        message: String,
    },
    #[error("{message}")]
    QuestionNotFoundError {
        #[serde(rename = "requestID")]
        request_id: String,
        message: String,
    },
    #[error("{message}")]
    ForbiddenError { message: String },
    #[error("{message}")]
    PtyNotFoundError {
        #[serde(rename = "ptyID")]
        pty_id: String,
        message: String,
    },
}

impl ApiError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            ApiError::InvalidRequestError { .. } => ErrorKind::InvalidRequest,
            ApiError::UnauthorizedError { .. } => ErrorKind::Unauthorized,
            ApiError::ConflictError { .. } => ErrorKind::Conflict,
            ApiError::ServiceUnavailableError { .. } => ErrorKind::ServiceUnavailable,
            ApiError::UnknownError { .. } => ErrorKind::Unknown,
            ApiError::ProviderNotFoundError { .. } => ErrorKind::ProviderNotFound,
            ApiError::SessionNotFoundError { .. } => ErrorKind::SessionNotFound,
            ApiError::MessageNotFoundError { .. } => ErrorKind::MessageNotFound,
            ApiError::InvalidCursorError { .. } => ErrorKind::InvalidCursor,
            ApiError::PermissionNotFoundError { .. } => ErrorKind::PermissionNotFound,
            ApiError::QuestionNotFoundError { .. } => ErrorKind::QuestionNotFound,
            ApiError::ForbiddenError { .. } => ErrorKind::Forbidden,
            ApiError::PtyNotFoundError { .. } => ErrorKind::PtyNotFound,
        }
    }

    pub fn status(&self) -> u16 {
        self.kind().status()
    }

    /// Schema-rejection error as produced by `packages/server/src/middleware/schema-error.ts`:
    /// reason truncated to 1024 UTF-16 units with a `... (N more chars)` suffix.
    pub fn schema_rejection(reason: &str, kind: &str) -> Self {
        const LIMIT: usize = 1024;
        let units: Vec<u16> = reason.encode_utf16().collect();
        let message = if units.len() <= LIMIT {
            reason.to_string()
        } else {
            format!(
                "{}... ({} more chars)",
                String::from_utf16_lossy(&units[..LIMIT]),
                units.len() - LIMIT
            )
        };
        ApiError::InvalidRequestError {
            message,
            kind: Some(kind.to_string()),
            field: None,
        }
    }

    /// `Authorization` middleware rejection. Server adds `www-authenticate: Basic realm="Secure Area"`.
    pub fn unauthorized() -> Self {
        ApiError::UnauthorizedError {
            message: "Authentication required".to_string(),
        }
    }
}

/// `WWW-Authenticate` challenge sent with 401 by the current and legacy authorization middleware.
pub const WWW_AUTHENTICATE: &str = "Basic realm=\"Secure Area\"";
