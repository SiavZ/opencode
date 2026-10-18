//! Purpose: narrow git contract used by project identity and the local worktree adapter, plus the
//! `git` subprocess implementation (ports of packages/core/src/git.ts `repo.discover`, `remote.get`,
//! `history.rootCommits` and the raw `git` helper of packages/opencode/src/worktree/index.ts).
//! Scope: trait, value types and `SubprocessGit`.
//! Uses:
//! rust/crates/
//! `-- locations/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/project.rs
//! |-- locations/src/workspace/worktree.rs
//! `-- locations/tests/project.rs
//! Invariants: discovery walks up looking for `.git` from the input path (file or directory) and
//! returns None when any of `--git-dir`/`--git-common-dir` fails; relative rev-parse output resolves
//! against the directory containing `.git`; spawn failures read as exit code 1 with empty output
//! (`Effect.catch(() => ({ code: 1, text: "", stderr: "" }))`); stdin is closed.
//! Contract tests: locations/tests/project.rs

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;

/// `Git.Repository`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub worktree: PathBuf,
    pub git_directory: PathBuf,
    pub common_directory: PathBuf,
}

/// Output of one git invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitOutput {
    pub code: i32,
    pub text: String,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// `stderr || text || fallback`, the baseline error message order.
    pub fn message(&self, fallback: &str) -> String {
        [self.stderr.as_str(), self.text.as_str()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    }
}

#[async_trait]
pub trait GitRunner: Send + Sync + std::fmt::Debug {
    /// Run `git args` in `cwd`. Never fails: spawn errors read as code 1.
    async fn run(&self, cwd: &Path, args: &[&str]) -> GitOutput;

    /// `Git.repo.discover`.
    async fn discover(&self, input: &Path) -> Option<Repository> {
        let dotgit = up(input, ".git").await?;
        let cwd = dotgit.parent()?.to_path_buf();
        let top = self.run(&cwd, &["rev-parse", "--show-toplevel"]).await;
        let git_dir = self.run(&cwd, &["rev-parse", "--git-dir"]).await;
        let common = self.run(&cwd, &["rev-parse", "--git-common-dir"]).await;
        if !git_dir.ok() || !common.ok() {
            return None;
        }
        Some(Repository {
            worktree: if top.ok() {
                resolve_path(&cwd, &top.text)
            } else {
                cwd.clone()
            },
            git_directory: resolve_path(&cwd, &git_dir.text),
            common_directory: resolve_path(&cwd, &common.text),
        })
    }

    /// `Git.remote.get(repo, "origin")`.
    async fn remote_url(&self, repo: &Repository, name: &str) -> Option<String> {
        let out = self.run(&repo.worktree, &["remote", "get-url", name]).await;
        if !out.ok() {
            return None;
        }
        let value = out.text.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    /// `Git.history.rootCommits`: sorted root commit hashes, empty when HEAD is unborn.
    async fn root_commits(&self, repo: &Repository) -> Vec<String> {
        let out = self
            .run(&repo.worktree, &["rev-list", "--max-parents=0", "HEAD"])
            .await;
        if !out.ok() {
            return Vec::new();
        }
        let mut roots: Vec<String> = out
            .text
            .split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        roots.sort();
        roots
    }
}

/// `git` subprocess runner (`ChildProcess.make("git", args, { cwd, extendEnv: true, stdin: "ignore" })`).
#[derive(Debug, Clone, Default)]
pub struct SubprocessGit {
    /// Extra environment, e.g. `GIT_CONFIG_GLOBAL` in tests.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl GitRunner for SubprocessGit {
    async fn run(&self, cwd: &Path, args: &[&str]) -> GitOutput {
        let mut command = tokio::process::Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        match command.output().await {
            Ok(output) => GitOutput {
                code: output.status.code().unwrap_or(1),
                text: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(error) => GitOutput {
                code: 1,
                text: String::new(),
                stderr: error.to_string(),
            },
        }
    }
}

/// `FileSystem.up({ targets: [target], start })[0]`.
async fn up(start: &Path, target: &str) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(target);
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// `resolvePath(cwd, value)` from git.ts.
pub fn resolve_path(cwd: &Path, value: &str) -> PathBuf {
    let trimmed = value.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return cwd.to_path_buf();
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return normalize(path);
    }
    normalize(&cwd.join(path))
}

/// Lexical `path.normalize` (no symlink resolution).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
