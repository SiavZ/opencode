//! Purpose: crate root of the native embedded SDK: subprocess server/TUI launcher and the opt-in
//! in-process composition facade. Never part of the portable client graph.
//! Scope: process-level host helpers for SDK consumers.
//! Uses:
//! rust/crates/
//! |-- sdk-embedded/src/embedded.rs
//! |-- sdk-embedded/src/launcher.rs
//! `-- sdk-embedded/src/process.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- sdk-embedded/tests/embedded.rs
//! `-- sdk-embedded/tests/launcher.rs
//! Invariants: launcher only spawns the configured program, embedded composition is behind the
//! `embedded` feature and reports `EmbedError::NotYetPorted` until Server and Runtime exist.
//! Contract tests: sdk-embedded/tests/launcher.rs, sdk-embedded/tests/embedded.rs

#[cfg(feature = "embedded")]
pub mod embedded;
pub mod launcher;
pub mod process;

#[cfg(feature = "embedded")]
pub use embedded::{EmbedError, OpenCode};
pub use launcher::{
    LaunchError, ServerHandle, ServerOptions, TuiHandle, TuiOptions, create_opencode_server,
    create_opencode_tui,
};
