//! Purpose: crate root for the portable platform contract: filesystem identity and mutation,
//! processes, PTY sessions, watchers, git, snapshots and path containment. No native I/O.
//! Scope: traits, value types and pure path helpers shared by tools, config, storage and runtime.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/error.rs
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/path.rs
//! |-- platform-api/src/process.rs
//! |-- platform-api/src/pty.rs
//! |-- platform-api/src/snapshot.rs
//! `-- platform-api/src/watcher.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-05, consumers platform-native, tools, permissions, config, storage-sqlite)
//! Invariants: compiles for wasm32-unknown-unknown, never depends on native-only crates.
//! Contract tests: platform-api/src/path.rs

pub mod error;
pub mod fs;
pub mod git;
pub mod path;
pub mod process;
pub mod pty;
pub mod snapshot;
pub mod watcher;

pub use error::{PlatformError, PlatformErrorKind};
pub use tokio_util::sync::CancellationToken;
