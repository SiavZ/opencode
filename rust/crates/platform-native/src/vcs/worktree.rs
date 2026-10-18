//! Purpose: worktree service of packages/opencode/src/worktree/index.ts: unique slug naming under
//! `<data>/worktree/<project>`, create (`--no-checkout` then `reset --hard` populate), list, remove
//! (with stale-directory cleanup and branch deletion) and reset to the default branch.
//! Scope: git-side mechanics. Project sandbox registration, instance disposal, bus events and the
//! start-script lookup stay with the runtime caller, which receives typed results to act on.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/run.rs
//! |-- platform-native/src/vcs/tree.rs
//! `-- platform-native/src/vcs/worktree_reset.rs
//! External:
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/src/vcs/worktree_reset.rs
//! `-- platform-native/tests/worktree.rs
//! Invariants: a candidate never reuses an existing directory or `opencode/<name>` branch; removal
//! of an unknown directory only deletes that directory; branch deletion follows a successful remove.
//! Contract tests: platform-native/tests/worktree.rs

use super::run::{GitCli, GitOutput};
use super::tree::remove_path;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_NAME_ATTEMPTS: usize = 26;
const ADJECTIVES: &[&str] = &[
    "brave", "calm", "clever", "cosmic", "crisp", "curious", "eager", "gentle", "glowing", "happy",
    "hidden", "jolly", "kind", "lucky", "mighty", "misty", "neon", "nimble", "playful", "proud",
    "quick", "quiet", "shiny", "silent", "stellar", "sunny", "swift", "tidy", "witty",
];
const NOUNS: &[&str] = &[
    "cabin", "cactus", "canyon", "circuit", "comet", "eagle", "engine", "falcon", "forest",
    "garden", "harbor", "island", "knight", "lagoon", "meadow", "moon", "mountain", "nebula",
    "orchid", "otter", "panda", "pixel", "planet", "river", "rocket", "sailor", "squid", "star",
    "tiger", "wizard", "wolf",
];

/// `Worktree` wire shape (`Info`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    pub directory: String,
}

/// Worktree failures, tagged like the baseline error classes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorktreeError {
    #[error("{0}")]
    NotGit(String),
    #[error("{0}")]
    NameGenerationFailed(String),
    #[error("{0}")]
    CreateFailed(String),
    #[error("{0}")]
    RemoveFailed(String),
    #[error("{0}")]
    ResetFailed(String),
    #[error("{0}")]
    ListFailed(String),
}

/// `Slug.create()`: `adjective-noun`.
pub fn slug() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let adjective = ADJECTIVES[usize::from(bytes[0]) % ADJECTIVES.len()];
    let noun = NOUNS[usize::from(bytes[1]) % NOUNS.len()];
    format!("{adjective}-{noun}")
}

/// `slugify`: lowercase, non-alphanumerics to `-`, trimmed of leading/trailing dashes.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in input.trim().to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// `failedRemoves`: paths from `warning: failed to remove <path>: ...` lines.
pub fn failed_removes(chunks: &[&str]) -> Vec<String> {
    chunks
        .iter()
        .flat_map(|chunk| chunk.lines())
        .filter_map(|line| {
            let line = line.trim();
            let lower = line.to_lowercase();
            let rest = lower.strip_prefix("warning:")?.trim_start();
            rest.strip_prefix("failed to remove")?;
            let start = line.len() - rest.len() + "failed to remove".len();
            let tail = line[start..].trim_start();
            let end = tail.rfind(": ")?;
            let value = tail[..end].trim().trim_matches(['\'', '"']);
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

/// Parsed `worktree list --porcelain` entry with the raw `branch` ref.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListEntry {
    pub path: String,
    pub branch: Option<String>,
}

pub fn parse_list(text: &str) -> Vec<ListEntry> {
    let mut out: Vec<ListEntry> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(ListEntry {
                path: path.trim().to_string(),
                branch: None,
            });
            continue;
        }
        if let (Some(current), Some(branch)) = (out.last_mut(), line.strip_prefix("branch ")) {
            current.branch = Some(branch.trim().to_string());
        }
    }
    out
}

/// `canonical`: resolve, realpath when possible, lowercase on Windows.
pub fn canonical(input: &Path) -> PathBuf {
    let absolute = std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf());
    let real = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    if cfg!(windows) {
        return PathBuf::from(real.to_string_lossy().to_lowercase());
    }
    real
}

/// Worktree manager bound to one git project (`InstanceState.context`).
#[derive(Debug, Clone)]
pub struct Worktrees {
    pub cli: GitCli,
    /// `Global.Path.data`.
    pub data_dir: PathBuf,
    pub project_id: String,
    /// `ctx.worktree` (current instance worktree root).
    pub worktree: PathBuf,
    /// `ctx.project.worktree` (primary worktree).
    pub primary: PathBuf,
    /// `ctx.project.vcs === "git"`.
    pub vcs_git: bool,
}

impl Worktrees {
    pub(crate) async fn git(&self, cwd: &Path, args: &[&str]) -> GitOutput {
        self.cli.run(cwd, args).await
    }

    fn require_git(&self) -> Result<(), WorktreeError> {
        if self.vcs_git {
            return Ok(());
        }
        Err(WorktreeError::NotGit(
            "Worktrees are only supported for git projects".into(),
        ))
    }

    /// `makeWorktreeInfo`: pick a free name under `<data>/worktree/<project>`.
    pub async fn make_info(
        &self,
        name: Option<&str>,
        detached: bool,
    ) -> Result<WorktreeInfo, WorktreeError> {
        self.require_git()?;
        let root = self.data_dir.join("worktree").join(&self.project_id);
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| WorktreeError::CreateFailed(error.to_string()))?;
        let base = name.map(slugify).unwrap_or_default();
        for attempt in 0..MAX_NAME_ATTEMPTS {
            let name = match (base.is_empty(), attempt) {
                (true, _) => slug(),
                (false, 0) => base.clone(),
                (false, _) => format!("{base}-{}", slug()),
            };
            let branch = (!detached).then(|| format!("opencode/{name}"));
            let directory = root.join(&name);
            if tokio::fs::try_exists(&directory).await.unwrap_or(false) {
                continue;
            }
            if let Some(branch) = &branch {
                let reference = format!("refs/heads/{branch}");
                let args = ["show-ref", "--verify", "--quiet", &reference];
                if self.git(&self.worktree, &args).await.ok() {
                    continue;
                }
            }
            return Ok(WorktreeInfo {
                name,
                branch,
                directory: directory.to_string_lossy().into_owned(),
            });
        }
        Err(WorktreeError::NameGenerationFailed(
            "Failed to generate a unique worktree name".into(),
        ))
    }

    /// `setup` + populate half of `boot`: add the worktree without checkout, then `reset --hard`.
    /// The populate failure message is returned as `Ok(Some(message))` (baseline emits `Failed`).
    pub async fn create_from_info(
        &self,
        info: &WorktreeInfo,
    ) -> Result<Option<String>, WorktreeError> {
        let directory = info.directory.as_str();
        let args: Vec<&str> = match &info.branch {
            Some(branch) => vec!["worktree", "add", "--no-checkout", "-b", branch, directory],
            None => vec![
                "worktree",
                "add",
                "--no-checkout",
                "--detach",
                directory,
                "HEAD",
            ],
        };
        let created = self.git(&self.worktree, &args).await;
        if !created.ok() {
            return Err(WorktreeError::CreateFailed(first_of(
                &created,
                "Failed to create git worktree",
            )));
        }
        let populated = self.git(Path::new(directory), &["reset", "--hard"]).await;
        if !populated.ok() {
            return Ok(Some(first_of(&populated, "Failed to populate worktree")));
        }
        Ok(None)
    }

    /// `Worktree.list`: linked worktrees excluding the primary.
    pub async fn list(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        if !self.vcs_git {
            return Ok(Vec::new());
        }
        let result = self
            .git(&self.worktree, &["worktree", "list", "--porcelain"])
            .await;
        if !result.ok() {
            let message = first_of(&result, "Failed to read git worktrees");
            return Err(WorktreeError::ListFailed(message));
        }
        let primary = canonical(&self.primary);
        let primary_name = file_name(&primary).to_lowercase();
        Ok(parse_list(&result.text)
            .into_iter()
            .filter_map(|entry| {
                let directory = canonical(Path::new(&entry.path));
                if directory == primary {
                    return None;
                }
                let name = file_name(&directory).to_lowercase();
                let name = if name == primary_name {
                    directory.parent().map(file_name).unwrap_or_default()
                } else {
                    name
                };
                Some(WorktreeInfo {
                    name,
                    branch: entry
                        .branch
                        .map(|b| b.strip_prefix("refs/heads/").unwrap_or(&b).to_string()),
                    directory: directory.to_string_lossy().into_owned(),
                })
            })
            .collect())
    }

    pub(crate) fn locate(&self, entries: Vec<ListEntry>, directory: &Path) -> Option<ListEntry> {
        entries
            .into_iter()
            .find(|entry| !entry.path.is_empty() && canonical(Path::new(&entry.path)) == directory)
    }

    async fn stop_fsmonitor(&self, target: &Path) {
        if tokio::fs::try_exists(target).await.unwrap_or(false) {
            let _ = self.git(target, &["fsmonitor--daemon", "stop"]).await;
        }
    }

    /// `Worktree.remove`: force-remove the worktree, delete its directory and its branch.
    pub async fn remove(&self, directory: &Path) -> Result<bool, WorktreeError> {
        self.require_git()?;
        let directory = canonical(directory);
        let list_args = ["worktree", "list", "--porcelain"];
        let list = self.git(&self.worktree, &list_args).await;
        if !list.ok() {
            let message = first_of(&list, "Failed to read git worktrees");
            return Err(WorktreeError::RemoveFailed(message));
        }
        let Some(entry) = self.locate(parse_list(&list.text), &directory) else {
            if tokio::fs::try_exists(&directory).await.unwrap_or(false) {
                self.stop_fsmonitor(&directory).await;
                clean_directory(&directory).await?;
            }
            return Ok(true);
        };
        let path = PathBuf::from(&entry.path);
        self.stop_fsmonitor(&path).await;
        let removed = self
            .git(
                &self.worktree,
                &["worktree", "remove", "--force", &entry.path],
            )
            .await;
        if !removed.ok() {
            let next = self.git(&self.worktree, &list_args).await;
            if !next.ok() {
                let message = [&removed.stderr, &removed.text, &next.stderr, &next.text]
                    .into_iter()
                    .find(|text| !text.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "Failed to remove git worktree".into());
                return Err(WorktreeError::RemoveFailed(message));
            }
            if self.locate(parse_list(&next.text), &directory).is_some() {
                let message = first_of(&removed, "Failed to remove git worktree");
                return Err(WorktreeError::RemoveFailed(message));
            }
        }
        clean_directory(&path).await?;
        if let Some(branch) = entry.branch {
            let branch = branch.strip_prefix("refs/heads/").unwrap_or(&branch);
            let deleted = self.git(&self.worktree, &["branch", "-D", branch]).await;
            if !deleted.ok() {
                let message = first_of(&deleted, "Failed to delete worktree branch");
                return Err(WorktreeError::RemoveFailed(message));
            }
        }
        Ok(true)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `stderr || text || fallback` (untrimmed, like the worktree service).
pub(crate) fn first_of(result: &GitOutput, fallback: &str) -> String {
    [&result.stderr, &result.text]
        .into_iter()
        .find(|text| !text.is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// `cleanDirectory`: recursive forced removal with retries (5, or 50 on Windows, 100ms apart).
async fn clean_directory(target: &Path) -> Result<(), WorktreeError> {
    let attempts = if cfg!(windows) { 50 } else { 5 };
    for attempt in 0..attempts {
        match remove_path(target).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt == attempts - 1 => {
                return Err(WorktreeError::RemoveFailed(error.to_string()));
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    Ok(())
}
