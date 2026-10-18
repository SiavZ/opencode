//! Purpose: sharing domain rules for enterprise shares (`share`), the public share Durable Object
//! (`public`) and JS-compatible key collation (`collation`).
//! Scope: module root and the shared error type.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/sharing/collation.rs
//! |-- hosted-domain/src/sharing/public.rs
//! `-- hosted-domain/src/sharing/share.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/mod.rs
//! `-- hosted-domain/src/lib.rs
//! Invariants: owned by phase-13 only. Error messages are the baseline `Share.Errors` messages
//! (packages/enterprise/src/core/share.ts:215-231) because the support API returns them verbatim.
//! Contract tests: hosted-domain/tests/sharing_domain.rs

pub mod collation;
pub mod public;
pub mod share;

/// `Share.Errors` plus the internal failures the baseline surfaces as HTTP 500.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShareError {
    #[error("Share not found: {0}")]
    NotFound(String),
    #[error("Share secret invalid: {0}")]
    InvalidSecret(String),
    #[error("Share already exists: {0}")]
    AlreadyExists(String),
    /// Storage failures, malformed stored JSON, or a TypeError in the baseline.
    #[error("{0}")]
    Internal(String),
}
