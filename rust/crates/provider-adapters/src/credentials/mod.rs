//! Purpose: credential acquisition and refresh: API key/env resolution, OAuth PKCE/headless and
//! device flows (OpenAI, GitHub Copilot, GitLab, others), near-expiry refresh leases.
//! Scope: module root. Owner: child node p03.auth.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: secrets never logged, refresh is single-flight per credential.
//! Contract tests: provider-adapters/tests/credentials_*.rs
