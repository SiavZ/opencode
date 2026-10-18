//! Purpose: GitHub integration module root (events, auth, run orchestration, workflow
//! installer, triage tools). Children are added by the phase-14 GitHub leaves.
//! Scope: module namespace only.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ecosystem/src/lib.rs
//! Invariants: none at the namespace level.
//! Contract tests: ecosystem/tests/
pub mod auth;
pub mod run;
pub mod triage;
