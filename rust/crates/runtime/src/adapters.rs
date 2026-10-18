//! Purpose: one-line delegations from domain implementations to the ports their consumers
//! declare: `opencode_locations::ports` -> `opencode_server::services` (LocationResolver,
//! SessionPlacement, LegacyWorkspaces), and project resolution -> Session `SessionLocations`.
//! Scope: adapter structs only, no domain logic.
//! Uses:
//! rust/crates/
//! |-- locations/src/ports.rs
//! |-- locations/src/project.rs
//! |-- locations/src/workspace/mod.rs
//! |-- server/src/services/ports.rs
//! |-- session-runtime/src/revert.rs
//! `-- session-runtime/src/service.rs
//! External:
//! |-- async_trait::async_trait
//! `-- opencode_session_api::{SessionError, SessionResult}
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/process.rs
//! Invariants: errors keep their meaning across the boundary (location failures become server
//! `PortError` defects, project resolution failures become retryable Session storage failures).
//! Revert is reported as unavailable until the native snapshot adapter (p05.native-vcs) lands.
//! Contract tests: runtime/tests/process.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_locations::ports as location_ports;
use opencode_locations::workspace::{Info as WorkspaceRecord, Target};
use opencode_locations::{LocationError, ProjectResolver};
use opencode_schema::location;
use opencode_schema::values::AbsolutePath;
use opencode_server::services::{
    LegacyWorkspaces, LocationInfo, LocationRef, LocationResolver, PortError, SessionPlacement,
    WorkspaceInfo, WorkspaceTarget,
};
use opencode_session_api::{SessionError, SessionResult};
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::service::{ResolvedProject, SessionLocations};

fn port_error(error: LocationError) -> PortError {
    PortError {
        message: error.to_string(),
    }
}

/// Server `LocationResolver` over any locations resolver (normally the registry).
pub struct ServerLocations(pub Arc<dyn location_ports::LocationResolver>);

#[async_trait]
impl LocationResolver for ServerLocations {
    async fn resolve(&self, location: &LocationRef) -> Result<LocationInfo, PortError> {
        self.0.resolve(location).await.map_err(port_error)
    }
}

/// Server `SessionPlacement` over the session table.
pub struct ServerPlacement(pub location_ports::SqlSessionPlacement);

#[async_trait]
impl SessionPlacement for ServerPlacement {
    async fn placement(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Option<String>)>, PortError> {
        location_ports::SessionPlacement::placement(&self.0, session_id)
            .await
            .map_err(port_error)
    }
}

/// Server `LegacyWorkspaces` over the control-plane workspace routing.
pub struct ServerWorkspaces(pub location_ports::WorkspaceRouting);

#[async_trait]
impl LegacyWorkspaces for ServerWorkspaces {
    fn configured_workspace_id(&self) -> Option<String> {
        location_ports::LegacyWorkspaces::configured_workspace_id(&self.0)
    }

    async fn session(&self, session_id: &str) -> Option<(String, Option<String>)> {
        location_ports::LegacyWorkspaces::session(&self.0, session_id).await
    }

    async fn workspace(&self, workspace_id: &str) -> Option<WorkspaceInfo> {
        location_ports::LegacyWorkspaces::workspace(&self.0, workspace_id)
            .await
            .map(|record| WorkspaceInfo { id: record.id })
    }

    async fn target(&self, workspace: &WorkspaceInfo) -> WorkspaceTarget {
        // The server only carries the id; reload the full record for the adapter lookup.
        let Some(record) = self.record(&workspace.id).await else {
            return WorkspaceTarget::Local {
                directory: String::new(),
            };
        };
        match location_ports::LegacyWorkspaces::target(&self.0, &record).await {
            Target::Local { directory } => WorkspaceTarget::Local { directory },
            Target::Remote { url, headers } => WorkspaceTarget::Remote { url, headers },
        }
    }

    async fn is_syncing(&self, workspace_id: &str) -> bool {
        location_ports::LegacyWorkspaces::is_syncing(&self.0, workspace_id).await
    }
}

impl ServerWorkspaces {
    async fn record(&self, workspace_id: &str) -> Option<WorkspaceRecord> {
        location_ports::LegacyWorkspaces::workspace(&self.0, workspace_id).await
    }
}

/// Session `SessionLocations`: `ProjectV2.resolve` plus Location-bound revert.
pub struct SessionProjects {
    pub projects: Arc<dyn ProjectResolver>,
}

#[async_trait]
impl SessionLocations for SessionProjects {
    async fn resolve_project(&self, directory: &AbsolutePath) -> SessionResult<ResolvedProject> {
        let resolved = self
            .projects
            .resolve(directory.as_str())
            .await
            .map_err(|error| SessionError::Storage {
                message: error.to_string(),
                retryable: true,
            })?;
        Ok(ResolvedProject {
            id: resolved.id,
            directory: AbsolutePath::new(resolved.directory.to_string_lossy()),
            vcs: resolved.vcs.map(|_| "git".to_owned()),
        })
    }

    fn revert(&self, _: &location::Ref) -> SessionResult<Revert> {
        Err(SessionError::Run {
            message: "NotYetPorted: Location snapshot service (p05.native-vcs)".to_owned(),
            retryable: false,
        })
    }
}
