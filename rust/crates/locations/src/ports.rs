//! Purpose: implementations of the Phase 02 server ports declared in rust/contracts/phase-90/
//! interfaces.md (`LocationResolver`, `SessionPlacement`, `LegacyWorkspaces`). `opencode-server`
//! depends on this crate, so the traits are published here with the same shapes and the server (or
//! runtime) wraps them in its own `services::ports` traits with a one-line delegation each.
//! Scope: traits plus SQL/registry-backed implementations.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/project.rs
//! |-- locations/src/registry.rs
//! |-- locations/src/workspace/mod.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/location.rs
//! |-- schema/src/values.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-api/src/records/codec.rs
//! External:
//! |-- async_trait::async_trait
//! |-- codec::directory::decode
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/tests/project.rs
//! |-- locations/tests/registry.rs
//! |-- locations/tests/registry_log.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/location.rs
//! `-- runtime/src/process.rs
//! Invariants: `Location.Info` is built exactly like `Location.layer(ref)` (packages/core/src/
//! location.ts:21-33): the ref's directory and workspaceID plus the resolved project id and
//! directory. Placement reads `session.directory`/`workspace_id`; missing rows are `None`.
//! Legacy routing never fails: unreadable sessions or workspaces read as `None`, adapter target
//! failures fall back to a local target in the workspace directory only when one is stored.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_schema::ids::{ProjectID, WorkspaceID};
use opencode_schema::location::{Info, InfoProject, Ref};
use opencode_schema::values::AbsolutePath;
use opencode_storage_api::records::codec;
use opencode_storage_api::{Database, DatabaseExt, StorageError};

use crate::error::{LocationError, Result};
use crate::project::ProjectResolver;
use crate::registry::{LocationRegistry, LocationServices};
use crate::workspace::{AdapterContext, Target, WorkspaceService};

/// `LocationServiceMap.get(ref)` resolved to `Location.Info`.
#[async_trait]
pub trait LocationResolver: Send + Sync {
    async fn resolve(&self, location: &Ref) -> Result<Info>;
}

/// Persisted Session placement (`SessionTable.directory`, `workspace_id`).
#[async_trait]
pub trait SessionPlacement: Send + Sync {
    async fn placement(&self, session_id: &str) -> Result<Option<(String, Option<String>)>>;
}

/// Legacy control-plane routing (`OPENCODE_WORKSPACE_ID`, `Workspace.get`, adapter target, sync).
#[async_trait]
pub trait LegacyWorkspaces: Send + Sync {
    fn configured_workspace_id(&self) -> Option<String>;
    async fn session(&self, session_id: &str) -> Option<(String, Option<String>)>;
    async fn workspace(&self, workspace_id: &str) -> Option<crate::workspace::Info>;
    async fn target(&self, workspace: &crate::workspace::Info) -> Target;
    async fn is_syncing(&self, workspace_id: &str) -> bool;
}

/// `Location.layer(ref)` over a project resolver.
pub struct ProjectLocationResolver {
    projects: Arc<dyn ProjectResolver>,
}

impl std::fmt::Debug for ProjectLocationResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectLocationResolver")
            .finish_non_exhaustive()
    }
}

impl ProjectLocationResolver {
    pub fn new(projects: Arc<dyn ProjectResolver>) -> Self {
        Self { projects }
    }
}

/// Build `Location.Info` from a ref and a resolved project.
pub fn location_info(location: &Ref, project: &ProjectID, directory: &str) -> Info {
    Info {
        directory: location.directory.clone(),
        workspace_id: location.workspace_id.clone(),
        project: InfoProject {
            id: project.clone(),
            directory: AbsolutePath::new(directory),
        },
    }
}

#[async_trait]
impl LocationResolver for ProjectLocationResolver {
    async fn resolve(&self, location: &Ref) -> Result<Info> {
        let resolved = self.projects.resolve(location.directory.as_str()).await?;
        Ok(location_info(
            location,
            &resolved.id,
            &resolved.directory.to_string_lossy(),
        ))
    }
}

/// A location bundle that exposes its `Location.Info` (the `Location.Service` member of the bundle).
pub trait HasLocationInfo {
    fn location_info(&self) -> &Info;
}

/// Resolve through the registry: builds (or reuses) the bundle like `LocationServiceMap.get`.
#[async_trait]
impl<S, G> LocationResolver for LocationRegistry<S, G>
where
    S: LocationServices + HasLocationInfo,
    G: Send + Sync + 'static,
{
    async fn resolve(&self, location: &Ref) -> Result<Info> {
        Ok(self.get(location).await?.location_info().clone())
    }
}

/// `SessionPlacement` over the `session` table.
#[derive(Clone)]
pub struct SqlSessionPlacement {
    db: Arc<dyn Database>,
}

impl std::fmt::Debug for SqlSessionPlacement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlSessionPlacement")
            .finish_non_exhaustive()
    }
}

impl SqlSessionPlacement {
    pub fn new(db: Arc<dyn Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SessionPlacement for SqlSessionPlacement {
    async fn placement(&self, session_id: &str) -> Result<Option<(String, Option<String>)>> {
        let id = session_id.to_owned();
        let row = self
            .db
            .read(move |sql| {
                sql.query_row(
                    "SELECT \"directory\", \"workspace_id\" FROM \"session\" WHERE \"id\" = ?1",
                    &[id.into()],
                )
            })
            .await?;
        row.map(|row| {
            Ok::<_, StorageError>((
                codec::directory::decode(&row, "directory")?,
                row.get::<Option<String>>("workspace_id")?,
            ))
        })
        .transpose()
        .map_err(LocationError::from)
    }
}

/// `LegacyWorkspaces` over the session table and the workspace service.
#[derive(Debug, Clone)]
pub struct WorkspaceRouting {
    placement: SqlSessionPlacement,
    workspaces: WorkspaceService,
    context: AdapterContext,
}

impl WorkspaceRouting {
    pub fn new(
        placement: SqlSessionPlacement,
        workspaces: WorkspaceService,
        context: AdapterContext,
    ) -> Self {
        Self {
            placement,
            workspaces,
            context,
        }
    }
}

#[async_trait]
impl LegacyWorkspaces for WorkspaceRouting {
    fn configured_workspace_id(&self) -> Option<String> {
        self.workspaces.options().configured_workspace_id.clone()
    }

    async fn session(&self, session_id: &str) -> Option<(String, Option<String>)> {
        self.placement.placement(session_id).await.ok().flatten()
    }

    async fn workspace(&self, workspace_id: &str) -> Option<crate::workspace::Info> {
        self.workspaces.get(workspace_id).await.ok().flatten()
    }

    async fn target(&self, workspace: &crate::workspace::Info) -> Target {
        let adapter = self
            .workspaces
            .adapters()
            .get(&workspace.project_id, &workspace.kind);
        let target = match adapter {
            Ok(adapter) => adapter.target(workspace, &self.context).await,
            Err(error) => Err(error),
        };
        target.unwrap_or_else(|error| {
            tracing::warn!(workspaceID = workspace.id, %error, "workspace target failed");
            Target::Local {
                directory: workspace.directory.clone().unwrap_or_default(),
            }
        })
    }

    async fn is_syncing(&self, workspace_id: &str) -> bool {
        self.workspaces.is_syncing(workspace_id)
    }
}

/// `Location.Ref` constructor (omitted workspace = implicit-local).
pub fn location_ref(directory: &str, workspace_id: Option<WorkspaceID>) -> Ref {
    Ref {
        directory: AbsolutePath::new(directory),
        workspace_id,
    }
}
