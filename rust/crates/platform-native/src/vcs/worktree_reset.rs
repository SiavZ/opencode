//! Purpose: `Worktree.reset` of packages/opencode/src/worktree/index.ts and the `Git.defaultBranch`
//! resolution it depends on (packages/opencode/src/git/index.ts:149-193).
//! Scope: reset a linked worktree to the default branch, sweep untracked files, refresh submodules.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/run.rs
//! |-- platform-native/src/vcs/tree.rs
//! `-- platform-native/src/vcs/worktree.rs
//! External:
//! |-- str::to_string
//! `-- str::trim
//! Used by:
//! rust/crates/
//! `-- platform-native/src/vcs/mod.rs
//! Invariants: the primary worktree is never reset; a reset that leaves local changes fails;
//! `clean -ffdx` failures retry once after pruning the reported paths inside the worktree only.
//! Contract tests: platform-native/tests/worktree.rs

use super::worktree::{WorktreeError, Worktrees, canonical, failed_removes, first_of, parse_list};
use std::path::Path;

/// `Git.Base`: branch name and the ref to reset to (`origin/main` or `main`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBranch {
    pub name: String,
    pub reference: String,
}

/// Global `-c` flags the legacy git service prepends (`cfg`).
const CFG: [&str; 11] = [
    "--no-optional-locks",
    "-c",
    "core.autocrlf=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.longpaths=true",
    "-c",
    "core.symlinks=true",
    "-c",
    "core.quotepath=false",
];

impl Worktrees {
    async fn legacy(&self, cwd: &Path, args: &[&str]) -> super::run::GitOutput {
        let full: Vec<&str> = CFG.iter().copied().chain(args.iter().copied()).collect();
        self.git(cwd, &full).await
    }

    async fn legacy_lines(&self, cwd: &Path, args: &[&str]) -> Vec<String> {
        self.legacy(cwd, args)
            .await
            .text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// `Git.defaultBranch`: remote HEAD of the primary remote, else configured init branch present
    /// locally, else `main`, else `master`.
    pub async fn default_branch(&self, cwd: &Path) -> Option<BaseBranch> {
        let remotes = self.legacy_lines(cwd, &["remote"]).await;
        let remote = if remotes.iter().any(|r| r == "origin") {
            Some("origin".to_string())
        } else if remotes.len() == 1 {
            remotes.first().cloned()
        } else if remotes.iter().any(|r| r == "upstream") {
            Some("upstream".to_string())
        } else {
            remotes.first().cloned()
        };
        if let Some(remote) = remote {
            let head = format!("refs/remotes/{remote}/HEAD");
            let result = self.legacy(cwd, &["symbolic-ref", &head]).await;
            if result.ok() {
                let text = result.text.trim();
                let reference = text.strip_prefix("refs/remotes/").unwrap_or(text);
                let name = reference
                    .strip_prefix(&format!("{remote}/"))
                    .unwrap_or_default();
                if !name.is_empty() {
                    return Some(BaseBranch {
                        name: name.to_string(),
                        reference: reference.to_string(),
                    });
                }
            }
        }
        let format = "--format=%(refname:short)";
        let list = self
            .legacy_lines(cwd, &["for-each-ref", format, "refs/heads"])
            .await;
        let configured = self.legacy(cwd, &["config", "init.defaultBranch"]).await;
        let configured = configured.text.trim();
        let pick = [configured, "main", "master"]
            .into_iter()
            .find(|name| !name.is_empty() && list.iter().any(|item| item == name))?;
        Some(BaseBranch {
            name: pick.to_string(),
            reference: pick.to_string(),
        })
    }

    async fn expect(&self, cwd: &Path, args: &[&str], fallback: &str) -> Result<(), WorktreeError> {
        let result = self.git(cwd, args).await;
        if result.ok() {
            return Ok(());
        }
        Err(WorktreeError::ResetFailed(first_of(&result, fallback)))
    }

    /// `sweep`: `clean -ffdx`, pruning paths git failed to remove and retrying once.
    async fn sweep(&self, root: &Path) -> super::run::GitOutput {
        let first = self.git(root, &["clean", "-ffdx"]).await;
        if first.ok() {
            return first;
        }
        let entries = failed_removes(&[&first.stderr, &first.text]);
        if entries.is_empty() {
            return first;
        }
        let base = canonical(root);
        for entry in entries {
            let target = canonical(&root.join(&entry));
            if target == base || !target.starts_with(&base) {
                continue;
            }
            let _ = super::tree::remove_path(&target).await;
        }
        self.git(root, &["clean", "-ffdx"]).await
    }

    /// `Worktree.reset`: hard reset a linked worktree to the default branch and clean it.
    /// Start scripts are the caller's responsibility (baseline forks them after success).
    pub async fn reset(&self, directory: &Path) -> Result<bool, WorktreeError> {
        if !self.vcs_git {
            return Err(WorktreeError::NotGit(
                "Worktrees are only supported for git projects".into(),
            ));
        }
        let directory = canonical(directory);
        if directory == canonical(&self.worktree) {
            return Err(WorktreeError::ResetFailed(
                "Cannot reset the primary workspace".into(),
            ));
        }
        let list = self
            .git(&self.worktree, &["worktree", "list", "--porcelain"])
            .await;
        if !list.ok() {
            let message = first_of(&list, "Failed to read git worktrees");
            return Err(WorktreeError::ResetFailed(message));
        }
        let entry = self
            .locate(parse_list(&list.text), &directory)
            .ok_or_else(|| WorktreeError::ResetFailed("Worktree not found".into()))?;
        let path = Path::new(&entry.path);
        let base = self
            .default_branch(&self.worktree)
            .await
            .ok_or_else(|| WorktreeError::ResetFailed("Default branch not found".into()))?;
        if base.reference != base.name
            && let Some((remote, branch)) = base.reference.split_once('/')
            && !remote.is_empty()
        {
            let fallback = format!("Failed to fetch {}", base.reference);
            self.expect(&self.worktree, &["fetch", remote, branch], &fallback)
                .await?;
        }
        let fallback = "Failed to reset worktree to target";
        self.expect(path, &["reset", "--hard", &base.reference], fallback)
            .await?;
        let cleaned = self.sweep(path).await;
        if !cleaned.ok() {
            let message = first_of(&cleaned, "Failed to clean worktree");
            return Err(WorktreeError::ResetFailed(message));
        }
        let steps: [(&[&str], &str); 3] = [
            (
                &["submodule", "update", "--init", "--recursive", "--force"],
                "Failed to update submodules",
            ),
            (
                &[
                    "submodule",
                    "foreach",
                    "--recursive",
                    "git",
                    "reset",
                    "--hard",
                ],
                "Failed to reset submodules",
            ),
            (
                &[
                    "submodule",
                    "foreach",
                    "--recursive",
                    "git",
                    "clean",
                    "-fdx",
                ],
                "Failed to clean submodules",
            ),
        ];
        for (args, fallback) in steps {
            self.expect(path, args, fallback).await?;
        }
        let status_args = ["-c", "core.fsmonitor=false", "status", "--porcelain=v1"];
        let status = self.git(path, &status_args).await;
        if !status.ok() {
            let message = first_of(&status, "Failed to read git status");
            return Err(WorktreeError::ResetFailed(message));
        }
        if !status.text.trim().is_empty() {
            return Err(WorktreeError::ResetFailed(format!(
                "Worktree reset left local changes:\n{}",
                status.text.trim()
            )));
        }
        Ok(true)
    }
}
