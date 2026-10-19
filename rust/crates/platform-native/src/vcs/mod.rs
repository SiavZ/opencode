//! Purpose: native git, snapshot, worktree and formatter adapters (Phase 05 child p05.native-vcs).
//! Scope: module declarations and re-exports only.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/change.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/format/mod.rs
//! |-- platform-native/src/vcs/repo.rs
//! |-- platform-native/src/vcs/run.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- platform-native/src/vcs/tree.rs
//! |-- platform-native/src/vcs/tree_restore.rs
//! |-- platform-native/src/vcs/worktree.rs
//! `-- platform-native/src/vcs/worktree_reset.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- platform-native/src/lib.rs
//! |-- platform-native/tests/git.rs
//! |-- platform-native/tests/git_support.rs
//! |-- platform-native/tests/snapshot.rs
//! |-- platform-native/tests/worktree.rs
//! |-- runtime/src/execution_support.rs
//! `-- runtime/src/location.rs
//! Invariants: every git child process goes through `platform_api::process::ProcessRunner`.
//! Contract tests: platform-native/tests/git.rs, platform-native/tests/snapshot.rs,
//! platform-native/tests/worktree.rs, platform-native/tests/format.rs

/// Working-tree change sets (`Git.change`).
pub mod change;
/// Typed VCS errors.
pub mod error;
/// Formatter table and service (`Format`).
pub mod format;
/// `platform_api::git::Git` adapter.
pub mod repo;
/// git CLI launcher.
pub mod run;
/// `platform_api::snapshot::Snapshots` adapter.
pub mod snapshot;
/// Index and tree operations (`Git.index`, `Git.tree`).
pub mod tree;
/// Preview, restore and checkout of tree paths.
pub mod tree_restore;
/// Worktree service (`Worktree`).
pub mod worktree;
/// `Worktree.reset` and default branch resolution.
pub mod worktree_reset;

pub use error::VcsError;
pub use repo::NativeGit;
pub use run::GitCli;
pub use snapshot::{NativeSnapshots, SnapshotConfig};
pub use worktree::{WorktreeError, WorktreeInfo, Worktrees};
