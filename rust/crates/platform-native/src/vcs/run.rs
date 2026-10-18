//! Purpose: `git` CLI invocation through the native `ProcessRunner` (baseline git.ts `run`,
//! `execute`, `repositoryArgs`, `repositoryOperation`), plus the per-git-dir keyed mutex.
//! Scope: command building, environment injection and result decoding. No git semantics.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! `-- platform-native/src/process/mod.rs
//! External:
//! `-- tokio::sync::Mutex
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/change.rs
//! |-- platform-native/src/vcs/repo.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- platform-native/src/vcs/tree.rs
//! |-- platform-native/src/vcs/tree_restore.rs
//! `-- platform-native/src/vcs/worktree.rs
//! Invariants: every git child inherits the parent environment plus the injected overrides; stdin
//! is closed unless bytes are supplied. Locks are keyed by git directory and never held across a
//! different repository's operation.
//! Contract tests: platform-native/tests/git.rs

use crate::process::NativeProcessRunner;
use opencode_platform_api::git::Repository;
use opencode_platform_api::process::{Command, ProcessError, ProcessRunner, RunOptions};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Decoded git result (`Result` in git.ts: exitCode, stdout text, stderr text).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitOutput {
    pub code: i32,
    pub text: String,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// `stderr.trim() || text.trim() || fallback`.
    pub fn message(&self, fallback: &str) -> String {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_string();
        }
        let text = self.text.trim();
        if !text.is_empty() {
            return text.to_string();
        }
        fallback.to_string()
    }
}

/// Shared git process launcher. Cheap to clone.
#[derive(Debug, Clone)]
pub struct GitCli {
    runner: Arc<dyn ProcessRunner>,
    env: BTreeMap<String, String>,
    locks: Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
}

impl Default for GitCli {
    fn default() -> Self {
        Self::new(Arc::new(NativeProcessRunner::new()))
    }
}

impl GitCli {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            env: BTreeMap::new(),
            locks: Arc::default(),
        }
    }

    /// Extra environment for every git child (tests isolate `GIT_CONFIG_GLOBAL` and `HOME`).
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env.extend(env);
        self
    }

    pub fn runner(&self) -> &Arc<dyn ProcessRunner> {
        &self.runner
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Spawn `git args` in `cwd`. Spawn failures surface as `ProcessError`.
    pub async fn exec<S: AsRef<str>>(
        &self,
        cwd: &Path,
        args: &[S],
        stdin: Option<Vec<u8>>,
        env: &[(&str, String)],
    ) -> Result<GitOutput, ProcessError> {
        let mut command = Command::new("git")
            .args(args.iter().map(|arg| arg.as_ref().to_string()))
            .cwd(cwd);
        command.env = self.env.clone();
        for (key, value) in env {
            command.env.insert((*key).to_string(), value.clone());
        }
        let result = self
            .runner
            .run(
                &command,
                RunOptions {
                    stdin,
                    ..RunOptions::default()
                },
            )
            .await?;
        Ok(GitOutput {
            code: result.exit_code,
            text: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        })
    }

    /// git.ts `run`: spawn failures collapse to exit 1 with empty output.
    pub async fn run<S: AsRef<str>>(&self, cwd: &Path, args: &[S]) -> GitOutput {
        self.exec(cwd, args, None, &[])
            .await
            .unwrap_or_else(|error| GitOutput {
                code: 1,
                text: String::new(),
                stderr: error.to_string(),
            })
    }

    /// `repositoryArgs`: `--git-dir <gitdir> --work-tree <worktree> ...args`, run in the worktree.
    pub async fn repo<S: AsRef<str>>(
        &self,
        repository: &Repository,
        args: &[S],
        stdin: Option<Vec<u8>>,
        env: &[(&str, String)],
    ) -> Result<GitOutput, ProcessError> {
        let mut full = vec![
            "--git-dir".to_string(),
            repository.git_dir.to_string_lossy().into_owned(),
            "--work-tree".to_string(),
            repository.root.to_string_lossy().into_owned(),
        ];
        full.extend(args.iter().map(|arg| arg.as_ref().to_string()));
        self.exec(&repository.root, &full, stdin, env).await
    }

    /// KeyedMutex over the repository git directory (`locked` in git.ts).
    pub fn lock(&self, key: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        locks.entry(key.to_path_buf()).or_default().clone()
    }
}

/// `resolvePath`: strip trailing newlines, resolve relative output against `cwd`.
pub fn resolve_path(cwd: &Path, value: &str) -> PathBuf {
    let trimmed = value.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return cwd.to_path_buf();
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return normalize(candidate);
    }
    normalize(&cwd.join(candidate))
}

/// Lexical normalization (`path.normalize`/`path.resolve`): drops `.` and folds `..`.
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
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

/// NUL-joined stdin payload with trailing NUL (`paths.join("\0") + "\0"`).
pub fn nul_join<S: AsRef<str>>(items: &[S]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        out.extend_from_slice(item.as_ref().as_bytes());
        out.push(0);
    }
    out
}

/// Split NUL-separated git output dropping empty entries.
pub fn nul_split(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}
