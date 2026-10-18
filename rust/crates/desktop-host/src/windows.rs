//! Purpose: window registry, persisted state, navigation and recovery policy.
//! Scope: module root, re-exports child modules. Does not own a native toolkit.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/windows/policy.rs
//! |-- desktop-host/src/windows/registry.rs
//! `-- desktop-host/src/windows/state.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! Invariants: see child modules.
//! Contract tests: desktop-host/tests/windows*.rs

pub mod policy;
pub mod registry;
pub mod state;
