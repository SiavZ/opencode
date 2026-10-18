//! Purpose: working-tree change sets of packages/core/src/git.ts (`change.capture/apply/discard`):
//! binary patches of tracked changes against HEAD plus untracked files, scoped to a sub-path.
//! Scope: free functions over `GitCli`, run in the repository worktree (not a separate git dir).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-native/src/vcs/error.rs
//! `-- platform-native/src/vcs/run.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/mod.rs
//! `-- platform-native/tests/git.rs
//! Invariants: a captured change set re-applies with `git apply` in a clean checkout of HEAD;
//! discard never touches paths outside the scope.
//! Contract tests: platform-native/tests/git.rs

use super::error::{PatchOperation, VcsError};
use super::run::{GitCli, GitOutput, nul_split};
use opencode_platform_api::git::Repository;
use std::path::Path;

/// `path.relative(worktree, path)` with forward slashes, `.` for the root.
fn scope_of(repository: &Repository, path: &Path) -> String {
    let relative = path
        .strip_prefix(&repository.root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    if relative.is_empty() {
        ".".to_string()
    } else {
        relative
    }
}

async fn exec(
    cli: &GitCli,
    operation: PatchOperation,
    cwd: &Path,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<GitOutput, VcsError> {
    cli.exec(cwd, args, stdin, &[])
        .await
        .map_err(|error| VcsError::patch(operation, error.to_string()))
}

/// `Git.change.capture`: `diff --binary HEAD` plus a `/dev/null` diff per untracked file.
pub async fn capture(
    cli: &GitCli,
    repository: &Repository,
    path: &Path,
) -> Result<String, VcsError> {
    let op = PatchOperation::Capture;
    let scope = scope_of(repository, path);
    let root = &repository.root;
    let tracked = exec(
        cli,
        op,
        root,
        &["diff", "--binary", "HEAD", "--", &scope],
        None,
    )
    .await?;
    if !tracked.ok() {
        let message = tracked.message("Failed to capture tracked changes");
        return Err(VcsError::patch(op, message));
    }
    let args = [
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        &scope,
    ];
    let untracked = exec(cli, op, root, &args, None).await?;
    if !untracked.ok() {
        let message = untracked.message("Failed to list untracked changes");
        return Err(VcsError::patch(op, message));
    }
    let mut parts = vec![tracked.text];
    for file in nul_split(&untracked.text) {
        let args = ["diff", "--binary", "--no-index", "--", "/dev/null", &file];
        let result = exec(cli, op, root, &args, None).await?;
        // git diff --no-index exits 1 when differences were found.
        if result.code != 0 && result.code != 1 {
            let fallback = format!("Failed to capture untracked change: {file}");
            return Err(VcsError::patch(op, result.message(&fallback)));
        }
        parts.push(result.text);
    }
    Ok(parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `Git.change.apply`: `git apply -` in `path`.
pub async fn apply(cli: &GitCli, path: &Path, changes: &str) -> Result<(), VcsError> {
    let op = PatchOperation::Apply;
    let stdin = Some(changes.as_bytes().to_vec());
    let result = exec(cli, op, path, &["apply", "-"], stdin).await?;
    if result.ok() {
        return Ok(());
    }
    Err(VcsError::patch(
        op,
        result.message("Failed to apply changes"),
    ))
}

/// Index handling for `discard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMode {
    Preserve,
    Reset,
}

/// `Git.change.discard`: restore tracked files under the scope and optionally clean untracked.
pub async fn discard(
    cli: &GitCli,
    repository: &Repository,
    path: &Path,
    index: IndexMode,
    remove_untracked: bool,
) -> Result<(), VcsError> {
    let op = PatchOperation::Reset;
    let scope = scope_of(repository, path);
    let root = &repository.root;
    let args: Vec<&str> = match index {
        IndexMode::Reset => vec!["checkout", "HEAD", "--", &scope],
        IndexMode::Preserve => vec!["checkout", "--", &scope],
    };
    let restore = exec(cli, op, root, &args, None).await?;
    if !restore.ok() {
        let message = restore.message("Failed to restore tracked changes");
        return Err(VcsError::patch(op, message));
    }
    if !remove_untracked {
        return Ok(());
    }
    let clean = exec(cli, op, root, &["clean", "-fd", "--", &scope], None).await?;
    if clean.ok() {
        return Ok(());
    }
    Err(VcsError::patch(
        op,
        clean.message("Failed to clean untracked changes"),
    ))
}
