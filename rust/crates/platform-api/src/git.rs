//! Purpose: git repository contract used by snapshots, worktrees, projects and VCS status
//! (subset of packages/core/src/git.ts Interface).
//! Scope: trait and values.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! `-- platform-api/src/snapshot.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! Invariants: every git invocation goes through ProcessRunner with bounded capture.
//! Contract tests: platform-native/tests/git.rs

use crate::process::ProcessError;
use crate::snapshot::FileDiff;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// Worktree root.
    pub root: PathBuf,
    /// The `.git` directory (or file target for linked worktrees).
    pub git_dir: PathBuf,
    /// Common dir shared by linked worktrees.
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error(transparent)]
    Process(#[from] ProcessError),
    #[error("git worktree: {0}")]
    Worktree(String),
    #[error("git patch: {0}")]
    Patch(String),
}

#[async_trait]
pub trait Git: Send + Sync + std::fmt::Debug {
    async fn discover(&self, path: &std::path::Path) -> Option<Repository>;
    async fn head(&self, repo: &Repository) -> Option<String>;
    async fn branch(&self, repo: &Repository) -> Option<String>;
    async fn root_commits(&self, repo: &Repository) -> Vec<String>;
    async fn remote_url(&self, repo: &Repository, name: Option<&str>) -> Option<String>;
    async fn worktree_create(
        &self,
        repo: &Repository,
        path: &std::path::Path,
        branch: &str,
    ) -> Result<Repository, GitError>;
    async fn worktree_remove(
        &self,
        repo: &Repository,
        path: &std::path::Path,
        force: bool,
    ) -> Result<(), GitError>;
    async fn worktree_list(&self, repo: &Repository) -> Result<Vec<Worktree>, GitError>;
    async fn diff_trees(
        &self,
        repo: &Repository,
        from: &str,
        to: &str,
        context: Option<u32>,
    ) -> Result<Vec<FileDiff>, GitError>;
}
