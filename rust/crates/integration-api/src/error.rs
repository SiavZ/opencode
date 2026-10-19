//! Purpose: shared typed error for integration services, preserving retry/cancel/not-found classes.
//! Scope: error values only.
//! Uses:
//! `-- none
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-adapters/src/mcp/service_api.rs
//! |-- integration-adapters/src/mcp/service_auth.rs
//! |-- integration-adapters/tests/mcp_oauth.rs
//! |-- integration-adapters/tests/mcp_stdio.rs
//! |-- integration-api/src/language.rs
//! |-- integration-api/src/lib.rs
//! |-- integration-api/src/mcp.rs
//! `-- integration-api/src/plugin.rs
//! Invariants: NotYetPorted names the exact missing behavior, never used as a silent success.
//! Contract tests: integration-api/src/mcp.rs

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IntegrationError {
    /// Baseline `MCP.NotFoundError` and unknown LSP/plugin names.
    #[error("{kind} not found: {name}")]
    NotFound { kind: &'static str, name: String },
    /// Baseline `MCPFailed` and LSP `LSPInitializeError`.
    #[error("{name} failed: {message}")]
    Failed { name: String, message: String },
    #[error("request timed out after {ms}ms")]
    Timeout { ms: u64 },
    #[error("cancelled")]
    Cancelled,
    #[error("invalid input: {0}")]
    Validation(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("transport closed: {0}")]
    Closed(String),
    /// A typed gap: the behavior exists in the TypeScript baseline but is not ported yet.
    #[error("not yet ported: {0}")]
    NotYetPorted(&'static str),
}

impl IntegrationError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Closed(_))
    }
}
