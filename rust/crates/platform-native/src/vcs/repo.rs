//! Purpose: native `platform_api::git::Git` adapter (packages/core/src/git.ts repo/remote/history/
//! worktree families) plus the non-trait helpers `create`, `default_remote_branch` and typed worktree calls.
//! Scope: repository discovery, history queries, linked worktree lifecycle and tree diff dispatch.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/run.rs
//! `-- platform-native/src/vcs/tree.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- platform-native/tests/git.rs
//! `-- platform-native/tests/worktree.rs
//! Invariants: discovery never fails, it returns None; query helpers return None/empty on any
//! git failure; worktree errors carry `force_required` when git reports a dirty worktree.
//! Contract tests: platform-native/tests/git.rs, platform-native/tests/worktree.rs

use super::error::{Operation, VcsError, WorktreeOperation};
use super::run::{GitCli, resolve_path};
use super::tree;
use async_trait::async_trait;
use opencode_platform_api::git::{Git, GitError, Repository, Worktree};
use opencode_platform_api::snapshot::FileDiff;
use std::path::{Path, PathBuf};

/// Snapshot repository config written by `repo.create` (git.ts:381-394).
const CREATE_CONFIG: [(&str, &str); 8] = [
    ("core.autocrlf", "false"),
    ("core.longpaths", "true"),
    ("core.symlinks", "true"),
    ("core.fsmonitor", "false"),
    ("feature.manyFiles", "true"),
    ("index.version", "4"),
    ("index.threads", "true"),
    ("core.untrackedCache", "true"),
];

#[derive(Debug, Clone, Default)]
pub struct NativeGit {
    cli: GitCli,
}

impl NativeGit {
    pub fn new(cli: GitCli) -> Self {
        Self { cli }
    }

    pub fn cli(&self) -> &GitCli {
        &self.cli
    }

    /// `Git.repo.create`: init a separate git dir over `worktree`, apply snapshot config and, with
    /// a seed, share its objects through `objects/info/alternates` and copy its index.
    pub async fn create(
        &self,
        worktree: &Path,
        git_dir: &Path,
        seed: Option<&Repository>,
    ) -> Result<Repository, VcsError> {
        let failed = |message: &str| VcsError::operation(Operation::Create, message);
        tokio::fs::create_dir_all(git_dir)
            .await
            .map_err(|_| failed("Failed to create Git storage"))?;
        let repository = Repository {
            root: worktree.to_path_buf(),
            git_dir: git_dir.to_path_buf(),
            common_dir: git_dir.to_path_buf(),
        };
        tree::repo_op(&self.cli, Operation::Create, &repository, &["init"], None).await?;
        for (key, value) in CREATE_CONFIG {
            tree::repo_op(
                &self.cli,
                Operation::Create,
                &repository,
                &["config", key, value],
                None,
            )
            .await?;
        }
        let Some(seed) = seed else {
            return Ok(repository);
        };
        let info = git_dir.join("objects").join("info");
        tokio::fs::create_dir_all(&info)
            .await
            .map_err(|_| failed("Failed to configure shared Git objects"))?;
        let objects = format!("{}\n", seed.common_dir.join("objects").to_string_lossy());
        tokio::fs::write(info.join("alternates"), objects)
            .await
            .map_err(|_| failed("Failed to configure shared Git objects"))?;
        let _ = tokio::fs::copy(seed.git_dir.join("index"), git_dir.join("index")).await;
        Ok(repository)
    }

    /// `Git.history.defaultRemoteBranch`.
    pub async fn default_remote_branch(
        &self,
        repo: &Repository,
        remote: Option<&str>,
    ) -> Option<String> {
        let remote = remote.unwrap_or("origin");
        let result = self
            .cli
            .run(
                &repo.root,
                &["symbolic-ref", &format!("refs/remotes/{remote}/HEAD")],
            )
            .await;
        if !result.ok() {
            return None;
        }
        let text = result.text.trim();
        let name = text
            .strip_prefix(&format!("refs/remotes/{remote}/"))
            .unwrap_or(text);
        (!name.is_empty()).then(|| name.to_string())
    }

    async fn worktree_run(
        &self,
        operation: WorktreeOperation,
        cwd: &Path,
        args: &[&str],
    ) -> Result<String, VcsError> {
        let result = self
            .cli
            .exec(cwd, args, None, &[])
            .await
            .map_err(|error| VcsError::worktree(operation, error.to_string(), false))?;
        if result.ok() {
            return Ok(result.text);
        }
        let message = result.message("Git failed");
        let lower = message.to_lowercase();
        let force_required = operation == WorktreeOperation::Remove
            && (lower.contains("contains modified or untracked files")
                || lower.contains("is dirty"));
        Err(VcsError::worktree(operation, message, force_required))
    }

    /// `Git.worktree.create` with an optional new branch (`-b`), detached at HEAD otherwise.
    pub async fn create_worktree(
        &self,
        repo: &Repository,
        path: &Path,
        branch: Option<&str>,
    ) -> Result<Repository, VcsError> {
        let target = path.to_string_lossy();
        let args: Vec<&str> = match branch {
            Some(branch) => vec!["worktree", "add", "-b", branch, &target],
            None => vec!["worktree", "add", "--detach", &target, "HEAD"],
        };
        self.worktree_run(WorktreeOperation::Create, &repo.root, &args)
            .await?;
        self.discover(path).await.ok_or_else(|| {
            VcsError::worktree(
                WorktreeOperation::Create,
                "Created worktree could not be opened",
                false,
            )
        })
    }

    /// `Git.worktree.remove`, run from the common directory like the baseline.
    pub async fn remove_worktree(
        &self,
        repo: &Repository,
        path: &Path,
        force: bool,
    ) -> Result<(), VcsError> {
        let target = path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&target);
        self.worktree_run(WorktreeOperation::Remove, &repo.common_dir, &args)
            .await
            .map(|_| ())
    }

    /// `git worktree list --porcelain` parsed into path/branch/head records, main first.
    pub async fn list_worktrees(&self, repo: &Repository) -> Result<Vec<Worktree>, VcsError> {
        let text = self
            .worktree_run(
                WorktreeOperation::List,
                &repo.root,
                &["worktree", "list", "--porcelain"],
            )
            .await?;
        Ok(parse_worktrees(&repo.root, &text))
    }
}

/// Porcelain worktree records. `branch` keeps the full `refs/heads/...` name stripped to short.
pub fn parse_worktrees(cwd: &Path, text: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Worktree {
                path: resolve_path(cwd, path.trim()),
                branch: None,
                head: None,
            });
            continue;
        }
        let Some(current) = out.last_mut() else {
            continue;
        };
        if let Some(branch) = line.strip_prefix("branch ") {
            let branch = branch.trim();
            current.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).into());
        }
        if let Some(head) = line.strip_prefix("HEAD ") {
            current.head = Some(head.trim().to_string());
        }
    }
    out
}

/// `fs.up({ targets: [".git"], start })[0]`: nearest `.git` entry walking to the root.
fn find_dotgit(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|candidate| candidate.exists())
}

#[async_trait]
impl Git for NativeGit {
    async fn discover(&self, path: &Path) -> Option<Repository> {
        let dotgit = find_dotgit(path)?;
        let cwd = dotgit.parent()?.to_path_buf();
        let top = self.cli.run(&cwd, &["rev-parse", "--show-toplevel"]).await;
        let git_dir = self.cli.run(&cwd, &["rev-parse", "--git-dir"]).await;
        let common = self.cli.run(&cwd, &["rev-parse", "--git-common-dir"]).await;
        if !git_dir.ok() || !common.ok() {
            return None;
        }
        Some(Repository {
            root: if top.ok() {
                resolve_path(&cwd, &top.text)
            } else {
                cwd.clone()
            },
            git_dir: resolve_path(&cwd, &git_dir.text),
            common_dir: resolve_path(&cwd, &common.text),
        })
    }

    async fn head(&self, repo: &Repository) -> Option<String> {
        let result = self.cli.run(&repo.root, &["rev-parse", "HEAD"]).await;
        let text = result.text.trim();
        (result.ok() && !text.is_empty()).then(|| text.to_string())
    }

    async fn branch(&self, repo: &Repository) -> Option<String> {
        let args = ["symbolic-ref", "--quiet", "--short", "HEAD"];
        let result = self.cli.run(&repo.root, &args).await;
        let text = result.text.trim();
        (result.ok() && !text.is_empty()).then(|| text.to_string())
    }

    async fn root_commits(&self, repo: &Repository) -> Vec<String> {
        let args = ["rev-list", "--max-parents=0", "HEAD"];
        let result = self.cli.run(&repo.root, &args).await;
        if !result.ok() {
            return Vec::new();
        }
        let mut roots: Vec<String> = result
            .text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        roots.sort();
        roots
    }

    async fn remote_url(&self, repo: &Repository, name: Option<&str>) -> Option<String> {
        let args = ["remote", "get-url", name.unwrap_or("origin")];
        let result = self.cli.run(&repo.root, &args).await;
        let text = result.text.trim();
        (result.ok() && !text.is_empty()).then(|| text.to_string())
    }

    async fn worktree_create(
        &self,
        repo: &Repository,
        path: &Path,
        branch: &str,
    ) -> Result<Repository, GitError> {
        let branch = (!branch.is_empty()).then_some(branch);
        Ok(self.create_worktree(repo, path, branch).await?)
    }

    async fn worktree_remove(
        &self,
        repo: &Repository,
        path: &Path,
        force: bool,
    ) -> Result<(), GitError> {
        Ok(self.remove_worktree(repo, path, force).await?)
    }

    async fn worktree_list(&self, repo: &Repository) -> Result<Vec<Worktree>, GitError> {
        Ok(self.list_worktrees(repo).await?)
    }

    async fn diff_trees(
        &self,
        repo: &Repository,
        from: &str,
        to: &str,
        context: Option<u32>,
    ) -> Result<Vec<FileDiff>, GitError> {
        Ok(tree::diff(&self.cli, repo, from, to, context, None).await?)
    }
}
