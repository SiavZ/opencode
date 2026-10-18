//! Purpose: GitHub authentication for the Action and the hosted token exchange: OIDC
//! verification, GitHub App JWT/installation tokens, PAT permission checks, framework-agnostic
//! exchange route handlers and the Action-side token acquisition/revoke client. Does not own
//! Git configuration (github/run) or hosted route mounting (phase 13).
//! Scope: module namespace plus the shared error type.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/app.rs
//! |-- ecosystem/src/github/auth/client.rs
//! |-- ecosystem/src/github/auth/handlers.rs
//! `-- ecosystem/src/github/auth/oidc.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- ecosystem/src/github/mod.rs
//! Invariants: error messages never contain token or private-key material.
//! Contract tests: ecosystem/tests/github_auth_oidc.rs, ecosystem/tests/github_auth_exchange.rs,
//! ecosystem/tests/github_auth_client.rs

pub mod app;
pub mod client;
pub mod handlers;
pub mod oidc;

/// Errors from the GitHub auth module. Variants keep validation, permission, upstream and
/// configuration distinctions so callers can map them to the baseline statuses.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Token or claim failed verification (signature, issuer, audience, expiry, claim shape).
    #[error("{0}")]
    Invalid(String),
    /// Caller lacks the required repository permission.
    #[error("{0}")]
    PermissionDenied(String),
    /// Upstream (GitHub, OIDC provider) answered with a non-success status. `message` follows
    /// Octokit's `RequestError` message format.
    #[error("{message}")]
    Upstream { status: u16, message: String },
    /// Network or I/O failure talking to an upstream.
    #[error("{0}")]
    Transport(String),
    /// Missing or malformed local configuration (environment, key material, inputs).
    #[error("{0}")]
    Config(String),
}

impl AuthError {
    /// True for upstream 404, the case the installation lookup treats as "not installed".
    pub fn is_not_found(&self) -> bool {
        matches!(self, AuthError::Upstream { status: 404, .. })
    }
}
