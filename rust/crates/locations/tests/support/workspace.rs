//! Purpose: shared workspace-service fixtures: a recording `WorkspaceHost`, a configurable failing
//! host adapter (`Flaky`), service setup over a migrated database and adapter contexts.
//! Scope: test support submodule.
//! Uses:
//! rust/crates/
//! |-- locations/src/lib.rs
//! `-- locations/tests/support/mod.rs
//! External:
//! `-- opencode_event_store::SqlEventStore
//! Used by:
//! rust/crates/
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- locations/tests/workspace_sync.rs
//! Invariants: `Flaky` fails only where told to; the host records every session removal.
//! Contract tests: locations/tests/workspace_lifecycle.rs

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use opencode_event_store::SqlEventStore;
use opencode_locations::workspace::{
    AdapterContext, AdapterRegistry, CreateEnv, CreateInput, Info, ProjectContext, SyncTransport,
    Target, WorkspaceAdapter, WorkspaceBus, WorkspaceHost, WorkspaceOptions, WorkspaceService,
};
use opencode_locations::{LocationError, Result};
use opencode_storage_api::Database;

use super::insert_project;

pub const PROJECT: &str = "proj";

pub struct Host {
    pub removed: Mutex<Vec<String>>,
}

#[async_trait]
impl WorkspaceHost for Host {
    async fn auth_content(&self) -> Result<String> {
        Ok("{}".into())
    }
    async fn remove_session(&self, id: &str) -> Result<()> {
        self.removed.lock().unwrap().push(id.to_owned());
        if id == "ses_missing" {
            return Err(LocationError::NotFound(id.into()));
        }
        Ok(())
    }
}

pub fn host() -> Arc<Host> {
    Arc::new(Host {
        removed: Mutex::new(Vec::new()),
    })
}

pub async fn setup(
    db: Arc<dyn Database>,
    adapters: AdapterRegistry,
    transport: Arc<dyn SyncTransport>,
    host: Arc<dyn WorkspaceHost>,
    experimental: bool,
) -> (WorkspaceService, Arc<SqlEventStore>) {
    insert_project(&db, PROJECT, "/p").await;
    let events = Arc::new(SqlEventStore::new(db.clone()));
    let service = WorkspaceService::new(
        db,
        events.clone(),
        adapters,
        WorkspaceBus::default(),
        transport,
        host,
        WorkspaceOptions {
            experimental_workspaces: experimental,
            configured_workspace_id: None,
            timeout: Duration::from_secs(5),
        },
    );
    (service, events)
}

pub fn context(worktree: &str, git: bool) -> AdapterContext {
    AdapterContext {
        project: Some(ProjectContext {
            id: PROJECT.into(),
            worktree: worktree.into(),
            git,
        }),
        workspace_id: None,
    }
}

pub fn input(kind: &str) -> CreateInput {
    CreateInput {
        id: None,
        kind: kind.into(),
        branch: None,
        project_id: PROJECT.into(),
        extra: None,
    }
}

/// Adapter whose host create/remove can be told to fail.
pub struct Flaky {
    pub fail_create: bool,
    pub fail_remove: bool,
    pub removed: Mutex<u32>,
}

#[async_trait]
impl WorkspaceAdapter for Flaky {
    fn name(&self) -> &str {
        "Flaky"
    }
    fn description(&self) -> &str {
        "test host"
    }
    async fn configure(&self, info: Info, _: &AdapterContext) -> Result<Info> {
        Ok(Info {
            directory: Some("/nonexistent/flaky".into()),
            ..info
        })
    }
    async fn create(
        &self,
        _: &Info,
        env: &CreateEnv,
        _: Option<&Info>,
        _: &AdapterContext,
    ) -> Result<()> {
        assert_eq!(
            env["OPENCODE_EXPERIMENTAL_WORKSPACES"].as_deref(),
            Some("true")
        );
        assert_eq!(env["OPENCODE_AUTH_CONTENT"].as_deref(), Some("{}"));
        if self.fail_create {
            return Err(LocationError::Adapter {
                message: "host create failed".into(),
            });
        }
        Ok(())
    }
    async fn remove(&self, _: &Info, _: &AdapterContext) -> Result<()> {
        *self.removed.lock().unwrap() += 1;
        if self.fail_remove {
            return Err(LocationError::Adapter {
                message: "host remove failed".into(),
            });
        }
        Ok(())
    }
    async fn target(&self, info: &Info, _: &AdapterContext) -> Result<Target> {
        Ok(Target::Local {
            directory: info.directory.clone().unwrap(),
        })
    }
}

pub fn flaky(fail_create: bool, fail_remove: bool) -> Arc<Flaky> {
    Arc::new(Flaky {
        fail_create,
        fail_remove,
        removed: Mutex::new(0),
    })
}
