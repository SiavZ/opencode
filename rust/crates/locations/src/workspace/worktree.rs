//! Purpose: built-in `worktree` workspace adapter backed by real `git worktree` (ports of
//! packages/opencode/src/control-plane/adapters/worktree.ts and the `makeWorktreeInfo`, `candidate`,
//! `setup`, `boot`, `list`, `remove` paths of packages/opencode/src/worktree/index.ts).
//! Scope: `WorktreeAdapter` and its `WorkspaceAdapter` implementation.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/git.rs
//! |-- locations/src/project_store.rs
//! |-- locations/src/workspace/adapter.rs
//! |-- locations/src/workspace/slug.rs
//! `-- schema/src/ids.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::future::BoxFuture
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- slug::create
//! |-- str::to_owned
//! |-- str::trim
//! |-- tokio::fs::canonicalize
//! |-- tokio::fs::create_dir_all
//! |-- tokio::fs::remove_dir_all
//! |-- tokio::fs::try_exists
//! |-- tokio::time::sleep
//! |-- tokio_util::task::TaskTracker
//! `-- tracing::error
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- runtime/src/process.rs
//! Invariants: every method needs an instance context (`Worktree adapter requires an instance
//! context`) and a git project (`Worktrees are only supported for git projects`, except `list`, which
//! returns empty); worktrees live in `<data>/worktree/<projectID>/<name>`; creation is
//! `git worktree add --no-checkout` then sandbox registration, then a background, owned `reset --hard`
//! that emits `worktree.ready` or `worktree.failed`; removal disposes the location first, removes
//! the worktree with `--force`, verifies it is gone, deletes the directory and the branch.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use opencode_schema::ids::ProjectID;
use serde_json::json;
use tokio_util::task::TaskTracker;

use super::adapter::{
    AdapterContext, BusEvent, CreateEnv, Info, ListedInfo, ProjectContext, Target,
    WorkspaceAdapter, WorkspaceBus,
};
use super::slug;
use crate::error::{LocationError, Result};
use crate::git::GitRunner;
use crate::project_store::ProjectStore;

const MAX_NAME_ATTEMPTS: usize = 26;

/// Called before a worktree directory is removed (`InstanceStore.disposeDirectory`).
pub type DisposeDirectory = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

/// The `worktree` adapter.
#[derive(Clone)]
pub struct WorktreeAdapter {
    git: Arc<dyn GitRunner>,
    /// `Global.Path.data`.
    data_dir: PathBuf,
    projects: Option<ProjectStore>,
    bus: WorkspaceBus,
    dispose: Option<DisposeDirectory>,
    tasks: TaskTracker,
}

impl std::fmt::Debug for WorktreeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeAdapter")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl WorktreeAdapter {
    pub fn new(git: Arc<dyn GitRunner>, data_dir: PathBuf, bus: WorkspaceBus) -> Self {
        Self {
            git,
            data_dir,
            projects: None,
            bus,
            dispose: None,
            tasks: TaskTracker::new(),
        }
    }

    /// Register created worktrees as project sandboxes (`project.addSandbox`, errors ignored).
    pub fn with_projects(mut self, projects: ProjectStore) -> Self {
        self.projects = Some(projects);
        self
    }

    /// Dispose a location before its directory is removed (wire to `LocationRegistry::invalidate`).
    pub fn with_dispose(mut self, dispose: DisposeDirectory) -> Self {
        self.dispose = Some(dispose);
        self
    }

    /// Join background boots (`reset --hard` and ready/failed events).
    pub async fn wait_idle(&self) {
        self.tasks.close();
        self.tasks.wait().await;
        self.tasks.reopen();
    }

    /// `Worktree.makeWorktreeInfo({ detached: true })` (the adapter always detaches).
    async fn make_info(&self, project: &ProjectContext) -> Result<(String, String)> {
        let root = self.data_dir.join("worktree").join(&project.id);
        tokio::fs::create_dir_all(&root).await?;
        for _ in 0..MAX_NAME_ATTEMPTS {
            let name = slug::create();
            let directory = root.join(&name);
            if tokio::fs::try_exists(&directory).await? {
                continue;
            }
            return Ok((name, directory.to_string_lossy().into_owned()));
        }
        Err(LocationError::Validation(
            "Failed to generate a unique worktree name".into(),
        ))
    }

    async fn worktrees(&self, cwd: &Path) -> Result<Vec<Entry>> {
        let out = self
            .git
            .run(cwd, &["worktree", "list", "--porcelain"])
            .await;
        if !out.ok() {
            return Err(LocationError::Git {
                message: out.message("Failed to read git worktrees"),
            });
        }
        Ok(parse_worktree_list(&out.text))
    }

    fn emit(
        &self,
        directory: &str,
        project: &str,
        workspace: Option<&str>,
        payload: serde_json::Value,
    ) {
        self.bus.emit(BusEvent {
            directory: Some(directory.to_owned()),
            project: Some(project.to_owned()),
            workspace: workspace.map(str::to_owned),
            payload,
        });
    }
}

#[async_trait]
impl WorkspaceAdapter for WorktreeAdapter {
    fn name(&self) -> &str {
        "Worktree"
    }

    fn description(&self) -> &str {
        "Create a git worktree"
    }

    async fn configure(&self, info: Info, context: &AdapterContext) -> Result<Info> {
        let project = git_project(context)?;
        let (name, directory) = self.make_info(project).await?;
        Ok(Info {
            name,
            directory: Some(directory),
            ..info
        })
    }

    async fn create(
        &self,
        info: &Info,
        _env: &CreateEnv,
        _from: Option<&Info>,
        context: &AdapterContext,
    ) -> Result<()> {
        let project = git_project(context)?;
        let directory = config_directory(info)?;
        let cwd = Path::new(&project.worktree);
        let args: Vec<&str> = match info.branch.as_deref().filter(|b| !b.is_empty()) {
            Some(branch) => vec!["worktree", "add", "--no-checkout", "-b", branch, &directory],
            None => vec![
                "worktree",
                "add",
                "--no-checkout",
                "--detach",
                &directory,
                "HEAD",
            ],
        };
        let created = self.git.run(cwd, &args).await;
        if !created.ok() {
            return Err(LocationError::Git {
                message: created.message("Failed to create git worktree"),
            });
        }
        if let Some(projects) = &self.projects {
            let _ = projects
                .add_sandbox(&ProjectID::new(project.id.clone()), &directory)
                .await;
        }
        let adapter = self.clone();
        let project_id = project.id.clone();
        let workspace = context.workspace_id.clone();
        let name = info.name.clone();
        let branch = info.branch.clone().filter(|b| !b.is_empty());
        self.tasks.spawn(async move {
            let populated = adapter
                .git
                .run(Path::new(&directory), &["reset", "--hard"])
                .await;
            if !populated.ok() {
                let message = populated.message("Failed to populate worktree");
                tracing::error!(directory, message, "worktree checkout failed");
                adapter.emit(
                    &directory,
                    &project_id,
                    workspace.as_deref(),
                    json!({ "type": "worktree.failed", "properties": { "message": message } }),
                );
                return;
            }
            let mut properties = serde_json::Map::new();
            properties.insert("name".into(), json!(name));
            if let Some(branch) = branch {
                properties.insert("branch".into(), json!(branch));
            }
            adapter.emit(
                &directory,
                &project_id,
                workspace.as_deref(),
                json!({ "type": "worktree.ready", "properties": properties }),
            );
        });
        Ok(())
    }

    async fn list(&self, context: &AdapterContext) -> Result<Vec<ListedInfo>> {
        let project = require_instance(context)?;
        if !project.git {
            return Ok(Vec::new());
        }
        let primary = canonical(&project.worktree).await;
        let primary_name = file_name(&primary).to_lowercase();
        let mut out = Vec::new();
        for entry in self.worktrees(Path::new(&project.worktree)).await? {
            let directory = canonical(&entry.path).await;
            if directory == primary {
                continue;
            }
            let lower = file_name(&directory).to_lowercase();
            let name = if lower == primary_name {
                Path::new(&directory)
                    .parent()
                    .map(|p| file_name(&p.to_string_lossy()))
                    .unwrap_or_default()
            } else {
                lower
            };
            out.push(ListedInfo {
                kind: "worktree".into(),
                name,
                branch: entry
                    .branch
                    .map(|b| b.trim_start_matches("refs/heads/").to_owned()),
                directory: Some(directory),
                extra: None,
                project_id: project.id.clone(),
            });
        }
        Ok(out)
    }

    async fn remove(&self, info: &Info, context: &AdapterContext) -> Result<()> {
        let project = git_project(context)?;
        let input = config_directory(info)?;
        let directory = canonical(&input).await;
        let cwd = Path::new(&project.worktree);
        if let Some(dispose) = &self.dispose
            && directory != canonical(&project.worktree).await
        {
            dispose(input.clone()).await;
        }
        let entries = self.worktrees(cwd).await?;
        let Some(entry) = locate(&entries, &directory).await else {
            if tokio::fs::try_exists(&directory).await? {
                stop_fsmonitor(self.git.as_ref(), &directory).await;
                clean_directory(&directory).await?;
            }
            return Ok(());
        };
        if let Some(dispose) = &self.dispose {
            dispose(entry.path.clone()).await;
        }
        stop_fsmonitor(self.git.as_ref(), &entry.path).await;
        let removed = self
            .git
            .run(cwd, &["worktree", "remove", "--force", &entry.path])
            .await;
        if !removed.ok() {
            let next = self
                .git
                .run(cwd, &["worktree", "list", "--porcelain"])
                .await;
            if !next.ok() {
                let message = [&removed.stderr, &removed.text, &next.stderr, &next.text]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "Failed to remove git worktree".into());
                return Err(LocationError::Git { message });
            }
            if locate(&parse_worktree_list(&next.text), &directory)
                .await
                .is_some()
            {
                return Err(LocationError::Git {
                    message: removed.message("Failed to remove git worktree"),
                });
            }
        }
        clean_directory(&entry.path).await?;
        if let Some(branch) = entry
            .branch
            .as_deref()
            .map(|b| b.trim_start_matches("refs/heads/"))
        {
            let deleted = self.git.run(cwd, &["branch", "-D", branch]).await;
            if !deleted.ok() {
                return Err(LocationError::Git {
                    message: deleted.message("Failed to delete worktree branch"),
                });
            }
        }
        Ok(())
    }

    async fn target(&self, info: &Info, _context: &AdapterContext) -> Result<Target> {
        Ok(Target::Local {
            directory: config_directory(info)?,
        })
    }
}

#[derive(Debug, Clone)]
struct Entry {
    path: String,
    branch: Option<String>,
}

/// `parseWorktreeList`.
fn parse_worktree_list(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.split('\n').map(str::trim) {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.push(Entry {
                path: path.trim().to_owned(),
                branch: None,
            });
        } else if let (Some(branch), Some(current)) =
            (line.strip_prefix("branch "), entries.last_mut())
        {
            current.branch = Some(branch.trim().to_owned());
        }
    }
    entries
}

async fn locate(entries: &[Entry], directory: &str) -> Option<Entry> {
    for entry in entries {
        if canonical(&entry.path).await == directory {
            return Some(entry.clone());
        }
    }
    None
}

fn require_instance(context: &AdapterContext) -> Result<&ProjectContext> {
    context
        .project
        .as_ref()
        .ok_or_else(|| LocationError::Adapter {
            message: "Worktree adapter requires an instance context".into(),
        })
}

fn git_project(context: &AdapterContext) -> Result<&ProjectContext> {
    let project = require_instance(context)?;
    if !project.git {
        return Err(LocationError::NotGit);
    }
    Ok(project)
}

/// `decodeWorktreeConfig(info).directory`.
fn config_directory(info: &Info) -> Result<String> {
    info.directory.clone().ok_or_else(|| {
        LocationError::Validation("Expected string, got null at [\"directory\"]".into())
    })
}

/// `canonical`: resolve, realpath when it exists, normalize (lowercased on Windows).
async fn canonical(input: &str) -> String {
    let absolute = std::path::absolute(input).unwrap_or_else(|_| PathBuf::from(input));
    let real = tokio::fs::canonicalize(&absolute).await.unwrap_or(absolute);
    let normalized = crate::git::normalize(&real).to_string_lossy().into_owned();
    if cfg!(windows) {
        return normalized.to_lowercase();
    }
    normalized
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn stop_fsmonitor(git: &dyn GitRunner, target: &str) {
    if tokio::fs::try_exists(target).await.unwrap_or(false) {
        let _ = git
            .run(Path::new(target), &["fsmonitor--daemon", "stop"])
            .await;
    }
}

/// `cleanDirectory`: `rm -rf` with retries (5 on unix, 50 on Windows, 100ms apart).
async fn clean_directory(target: &str) -> Result<()> {
    let attempts = if cfg!(windows) { 50 } else { 5 };
    for attempt in 0..attempts {
        match tokio::fs::remove_dir_all(target).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt == attempts - 1 => {
                return Err(LocationError::Git {
                    message: error.to_string(),
                });
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    Ok(())
}
