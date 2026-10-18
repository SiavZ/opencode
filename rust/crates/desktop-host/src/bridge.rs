//! Purpose: sender-scoped capability bridge: IPC channel catalog, dispatcher and subscriptions.
//! Scope: module root, re-exports child modules. Does not perform platform side effects.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/bridge/catalog.rs
//! |-- desktop-host/src/bridge/dispatch.rs
//! `-- desktop-host/src/bridge/subscriptions.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! Invariants: wire channel names match packages/desktop/src/preload and main/ipc.ts exactly.
//! Contract tests: desktop-host/tests/bridge*.rs

pub mod catalog;
pub mod dispatch;
pub mod subscriptions;
