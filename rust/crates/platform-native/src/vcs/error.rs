//! Purpose: typed VCS errors mirroring git.ts `OperationError`, `WorktreeError`, `PatchError`, with
//! lossless conversion into `platform_api::git::GitError`.
//! Scope: error values only.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! `-- platform-api/src/process.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/change.rs
//! |-- platform-native/src/vcs/repo.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- platform-native/tests/git.rs
//! |-- platform-native/src/vcs/tree.rs
//! `-- platform-native/src/vcs/tree_restore.rs
//! Invariants: the message is the git stderr (trimmed) or stdout or a fixed fallback, never empty.
//! Contract tests: platform-native/tests/git.rs

use opencode_platform_api::git::GitError;
use opencode_platform_api::process::ProcessError;

/// `OperationError.operation` literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Create,
    Refresh,
    WriteTree,
    ListFiles,
    Diff,
    Restore,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Refresh => "refresh",
            Self::WriteTree => "write_tree",
            Self::ListFiles => "list_files",
            Self::Diff => "diff",
            Self::Restore => "restore",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeOperation {
    Create,
    Remove,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOperation {
    Capture,
    Apply,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VcsError {
    #[error("{message}")]
    Operation {
        operation: Operation,
        message: String,
    },
    #[error("{message}")]
    Worktree {
        operation: WorktreeOperation,
        message: String,
        force_required: bool,
    },
    #[error("{message}")]
    Patch {
        operation: PatchOperation,
        message: String,
    },
    #[error(transparent)]
    Process(#[from] ProcessError),
}

impl VcsError {
    pub fn operation(operation: Operation, message: impl Into<String>) -> Self {
        Self::Operation {
            operation,
            message: message.into(),
        }
    }

    pub fn worktree(
        operation: WorktreeOperation,
        message: impl Into<String>,
        force_required: bool,
    ) -> Self {
        Self::Worktree {
            operation,
            message: message.into(),
            force_required,
        }
    }

    pub fn patch(operation: PatchOperation, message: impl Into<String>) -> Self {
        Self::Patch {
            operation,
            message: message.into(),
        }
    }

    /// True for a worktree removal refused because the worktree has local changes.
    pub fn force_required(&self) -> bool {
        matches!(
            self,
            Self::Worktree {
                force_required: true,
                ..
            }
        )
    }
}

impl From<VcsError> for GitError {
    fn from(error: VcsError) -> Self {
        match error {
            VcsError::Process(error) => GitError::Process(error),
            VcsError::Worktree { message, .. } => GitError::Worktree(message),
            VcsError::Patch { message, .. } => GitError::Patch(message),
            VcsError::Operation { operation, message } => GitError::Process(ProcessError::Failed {
                command: format!("git {}", operation.as_str()),
                exit_code: None,
                stderr: None,
                detail: Some(message),
            }),
        }
    }
}
