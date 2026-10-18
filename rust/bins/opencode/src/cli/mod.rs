//! Purpose: command line root module: spec (clap tree), dispatch and command handlers.
//! Scope: process run.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/dispatch.rs
//! `-- opencode/src/cli/spec/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- opencode/src/main.rs
//! Invariants: stdout carries command output only, diagnostics go to stderr.
//! Contract tests: opencode/tests/
pub mod commands;
pub mod dispatch;
pub mod spec;
