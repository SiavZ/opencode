//! Purpose: platform adapters for the desktop host: process-tree control, proxy/CA client
//! construction, login-shell env capture, external app detection/opening and WSL servers.
//! Scope: native OS adapters only, no window or UI state.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/apps.rs
//! |-- desktop-host/src/platform/process_tree.rs
//! |-- desktop-host/src/platform/proxy.rs
//! |-- desktop-host/src/platform/shell_env.rs
//! `-- desktop-host/src/platform/wsl/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! Invariants: every adapter that spawns a process owns and joins it, or documents why it is detached.
//! Contract tests: desktop-host/tests/platform.rs, desktop-host/tests/wsl_servers.rs

pub mod apps;
pub mod process_tree;
pub mod proxy;
pub mod shell_env;
pub mod wsl;
