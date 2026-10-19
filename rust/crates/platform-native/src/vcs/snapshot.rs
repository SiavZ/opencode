//! Purpose: native `platform_api::snapshot::Snapshots` (packages/core/src/snapshot.ts): a separate
//! git dir under `<data>/snapshot/<project>/<sha1(worktree)>` sharing the project's objects, with
//! Location-scoped capture, files, diff, preview, restore, checkout, plus legacy `cleanup` (gc).
//! Scope: one instance per Location. Construction discovers the source repository once.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/repo.rs
//! `-- platform-native/src/vcs/tree.rs
//! External:
//! |-- async_trait::async_trait
//! |-- sha1::Digest
//! |-- sha1::Sha1
//! |-- tokio::fs::try_exists
//! |-- tracing::info
//! |-- tracing::warn
//! |-- tree::capture
//! |-- tree::checkout
//! |-- tree::diff
//! |-- tree::files
//! |-- tree::ignored
//! |-- tree::preview
//! `-- tree::restore
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/tests/snapshot.rs
//! `-- runtime/src/execution_support.rs
//! Invariants: capture is best effort (None when disabled, outside git or failed); untracked files
//! above 2 MiB and paths ignored by the source repository never enter a tree; restore/preview reject
//! paths escaping the project; nothing is written outside the injected data dir and the worktree.
//! Contract tests: platform-native/tests/snapshot.rs

use super::error::VcsError;
use super::repo::NativeGit;
use super::tree;
use async_trait::async_trait;
use opencode_platform_api::git::{Git, Repository};
use opencode_platform_api::snapshot::{FileDiff, SnapshotError, SnapshotOperation, Snapshots};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// `maximumUntrackedFileBytes` used by snapshot capture and preview.
pub const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;
/// Legacy snapshot gc prune window (opencode/src/snapshot/index.ts `prune`).
pub const PRUNE: &str = "7.days";

/// Inputs for one Location-scoped snapshot service.
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// `Global.data`; the snapshot git dir lives under `<data>/snapshot`.
    pub data_dir: PathBuf,
    pub project_id: String,
    /// `location.project.directory`.
    pub project_directory: PathBuf,
    /// `location.directory` (the capture scope).
    pub directory: PathBuf,
    /// `location.vcs?.type === "git"`.
    pub vcs_git: bool,
    /// `Config.latest(entries, "snapshots") !== false`.
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct NativeSnapshots {
    git: NativeGit,
    config: SnapshotConfig,
    source: Option<Repository>,
    worktree: PathBuf,
    git_dir: PathBuf,
}

/// `Hash.fast`: sha1 hex.
pub fn hash_fast(input: &str) -> String {
    Sha1::digest(input.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn fail(operation: SnapshotOperation, message: impl Into<String>) -> SnapshotError {
    SnapshotError {
        operation,
        message: message.into(),
    }
}

fn failure(operation: SnapshotOperation) -> impl Fn(VcsError) -> SnapshotError {
    move |error| fail(operation, error.to_string())
}

/// `FSUtil.contains(worktree, resolve(worktree, file))`, lexically.
fn escapes(file: &str) -> bool {
    let path = Path::new(file);
    if path.is_absolute() {
        return true;
    }
    let mut depth: i64 = 0;
    for component in path.components() {
        match component {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return true,
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

impl NativeSnapshots {
    /// Discover the source repository and derive the snapshot git dir (snapshot.ts layer body).
    pub async fn open(git: NativeGit, config: SnapshotConfig) -> Self {
        let source = git.discover(&config.project_directory).await;
        let worktree = match &source {
            Some(source) => real(&source.root),
            None => config.project_directory.clone(),
        };
        let git_dir = config
            .data_dir
            .join("snapshot")
            .join(&config.project_id)
            .join(hash_fast(&worktree.to_string_lossy()));
        Self {
            git,
            config,
            source,
            worktree,
            git_dir,
        }
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    fn enabled(&self) -> bool {
        self.config.vcs_git && self.config.enabled
    }

    fn scope(&self) -> Result<String, SnapshotError> {
        let directory = real(&self.config.directory);
        let relative = directory.strip_prefix(&self.worktree).map_err(|_| {
            fail(
                SnapshotOperation::Capture,
                "Location is outside the project",
            )
        })?;
        let text = relative.to_string_lossy().replace('\\', "/");
        Ok(if text.is_empty() { ".".into() } else { text })
    }

    async fn repository(&self, operation: SnapshotOperation) -> Result<Repository, SnapshotError> {
        let Some(source) = &self.source else {
            return Err(fail(operation, "Project is not a Git repository"));
        };
        if tokio::fs::try_exists(self.git_dir.join("HEAD"))
            .await
            .unwrap_or(false)
        {
            return Ok(Repository {
                root: self.worktree.clone(),
                git_dir: self.git_dir.clone(),
                common_dir: self.git_dir.clone(),
            });
        }
        self.git
            .create(&self.worktree, &self.git_dir, Some(source))
            .await
            .map_err(failure(operation))
    }

    async fn try_capture(&self) -> Result<String, SnapshotError> {
        let repo = self.repository(SnapshotOperation::Capture).await?;
        let scopes = [self.scope()?];
        tree::capture(
            self.git.cli(),
            &repo,
            &scopes,
            self.source.as_ref(),
            Some(MAX_UNTRACKED_BYTES),
        )
        .await
        .map_err(failure(SnapshotOperation::Capture))
    }

    /// Changed files between two trees, filtered by the source repository's ignore rules.
    async fn changed(
        &self,
        operation: SnapshotOperation,
        repo: &Repository,
        from: &str,
        to: &str,
    ) -> Result<(Vec<String>, BTreeSet<String>), SnapshotError> {
        let cli = self.git.cli();
        let files = tree::files(cli, repo, from, to)
            .await
            .map_err(failure(operation))?;
        let ignored = match &self.source {
            Some(source) => tree::ignored(cli, source, &files)
                .await
                .map_err(failure(operation))?,
            None => BTreeSet::new(),
        };
        Ok((files, ignored))
    }

    fn plan(
        operation: SnapshotOperation,
        files: &BTreeMap<String, String>,
    ) -> Result<(), SnapshotError> {
        match files.keys().find(|file| escapes(file)) {
            Some(file) => Err(fail(operation, format!("Path escapes the project: {file}"))),
            None => Ok(()),
        }
    }

    /// Legacy `Snapshot.cleanup`: `git gc --prune=7.days` on the snapshot git dir. Best effort.
    pub async fn cleanup(&self) {
        if !self.enabled() {
            return;
        }
        if !tokio::fs::try_exists(&self.git_dir).await.unwrap_or(false) {
            return;
        }
        let lock = self.git.cli().lock(&self.git_dir);
        let _guard = lock.lock().await;
        let repo = Repository {
            root: self.worktree.clone(),
            git_dir: self.git_dir.clone(),
            common_dir: self.git_dir.clone(),
        };
        let prune = format!("--prune={PRUNE}");
        match self.git.cli().repo(&repo, &["gc", &prune], None, &[]).await {
            Ok(out) if out.ok() => tracing::info!(prune = PRUNE, "snapshot cleanup"),
            Ok(out) => tracing::warn!(code = out.code, stderr = %out.stderr, "cleanup failed"),
            Err(error) => tracing::warn!(%error, "cleanup failed"),
        }
    }
}

#[async_trait]
impl Snapshots for NativeSnapshots {
    async fn capture(&self) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        match self.try_capture().await {
            Ok(tree) => Some(tree),
            Err(error) => {
                tracing::warn!(%error, "failed to capture snapshot");
                None
            }
        }
    }

    async fn files(&self, from: &str, to: &str) -> Result<Vec<String>, SnapshotError> {
        let op = SnapshotOperation::Files;
        let repo = self.repository(op).await?;
        let (files, ignored) = self.changed(op, &repo, from, to).await?;
        Ok(files.into_iter().filter(|f| !ignored.contains(f)).collect())
    }

    async fn diff(
        &self,
        from: &str,
        to: &str,
        context: Option<u32>,
        paths: Option<&[String]>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        let op = SnapshotOperation::Diff;
        let repo = self.repository(op).await?;
        let (files, ignored) = self.changed(op, &repo, from, to).await?;
        let selected: Vec<String> = paths
            .map(<[String]>::to_vec)
            .unwrap_or(files)
            .into_iter()
            .filter(|file| !ignored.contains(file))
            .collect();
        tree::diff(self.git.cli(), &repo, from, to, context, Some(&selected))
            .await
            .map_err(failure(op))
    }

    async fn preview(
        &self,
        files: &BTreeMap<String, String>,
        context: Option<u32>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        let op = SnapshotOperation::Preview;
        if !self.enabled() {
            return Err(fail(op, "Snapshots are disabled"));
        }
        let repo = self.repository(op).await?;
        Self::plan(op, files)?;
        let cli = self.git.cli();
        let scopes: Vec<String> = files.keys().cloned().collect();
        let current = tree::capture(
            cli,
            &repo,
            &scopes,
            self.source.as_ref(),
            Some(MAX_UNTRACKED_BYTES),
        )
        .await
        .map_err(failure(op))?;
        tree::preview(cli, &repo, &current, files, context)
            .await
            .map_err(failure(op))
    }

    async fn restore(&self, files: &BTreeMap<String, String>) -> Result<(), SnapshotError> {
        let op = SnapshotOperation::Restore;
        if !self.enabled() {
            return Err(fail(op, "Snapshots are disabled"));
        }
        let repo = self.repository(op).await?;
        Self::plan(op, files)?;
        tree::restore(self.git.cli(), &repo, files)
            .await
            .map_err(failure(op))
    }

    async fn checkout(&self, snapshot: &str) -> Result<(), SnapshotError> {
        let op = SnapshotOperation::Restore;
        let repo = self.repository(op).await?;
        tree::checkout(self.git.cli(), &repo, snapshot)
            .await
            .map_err(failure(op))
    }
}
