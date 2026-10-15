//! Purpose: Phase 11 connections spike exercising desktop-host public local spawn/health/stop
//! and remote connection against LOCAL temp fixtures. Proves process cleanup and identity.
//! Scope: throwaway spike, not production code. Uses tempdirs and bounded teardown.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/connections.rs
//! External:
//! |-- tokio
//! `-- tempfile
//! Used by:
//! `-- none, spike only
//! Invariants: no production data, no edits to connections owner files, bounded teardown.
//! Contract tests: tests/spawn_health_stop.rs

pub mod fixtures;
