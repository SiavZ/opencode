//! Purpose: crate root for native platform adapters implementing `opencode-platform-api` traits.
//! Scope: module declarations only. Each family is owned by one Phase 05 child node.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! |-- platform-native/src/process/mod.rs
//! |-- platform-native/src/pty/mod.rs
//! `-- platform-native/src/vcs/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: native-only, never a dependency of portable crates.
//! Contract tests: platform-native/tests/

/// Filesystem identity, mutation, locks, encoding detection and watchers (child p05.native-fs).
pub mod fs;
/// Process spawning, group/descendant termination and bounded capture (child p05.native-process).
pub mod process;
/// PTY sessions with bounded replay and cursor (child p05.native-process).
pub mod pty;
/// Git, snapshots, worktrees and formatters (child p05.native-vcs).
pub mod vcs;
