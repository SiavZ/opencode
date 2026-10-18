//! Purpose: workspace lifecycle service (port of packages/opencode/src/control-plane/workspace.ts
//! `create`, `list`, `syncList`, `get`, `remove`, `status`, `isSyncing`, `startWorkspaceSyncing`, and
//! the core facade packages/core/src/workspace.ts).
//! Scope: `WorkspaceService` over the `workspace`/`session` tables, adapters and the sync supervisor.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/workspace/adapter.rs
//! |-- locations/src/workspace/host.rs
//! |-- locations/src/workspace/slug.rs
//! |-- locations/src/workspace/sse.rs
//! |-- locations/src/workspace/sync.rs
//! `-- locations/src/workspace/worktree.rs
//! External:
//! |-- opencode_event_api::EventStore
//! |-- opencode_schema::ids::WorkspaceID
//! |-- opencode_storage_api::records::{Crud, WorkspaceRow}
//! |-- opencode_storage_api::{Database, DatabaseExt}
//! `-- tokio_util::task::TaskTracker
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- locations/tests/workspace_sync.rs
//! Invariants: `create` is not atomic, like the baseline: the row is inserted before the adapter
//! creates the host, and a host failure returns `CreateFailed { workspace_id }` with the row left in
//! place for `remove` to clean up. `remove` deletes top-level sessions first, stops sync, tolerates
//! adapter failures (logged), then deletes the row. Status changes emit `workspace.status` once per
//! distinct value. Every sync task is owned by the service and joined by `shutdown`.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

pub mod adapter;
pub mod host;
pub mod slug;
pub mod sse;
pub mod sync;
pub mod worktree;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use opencode_event_api::EventStore;
use opencode_schema::ids::WorkspaceID;
use opencode_schema::workspace::ConnectionState;
use opencode_storage_api::records::collation::locale_compare;
use opencode_storage_api::records::crud::now_ms;
use opencode_storage_api::records::{Crud, WorkspaceRow};
use opencode_storage_api::{Database, DatabaseExt, TxOptions};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub use adapter::{
    AdapterContext, AdapterEntry, AdapterRegistry, BusEvent, CreateEnv, Info, ListedInfo,
    ProjectContext, Target, WorkspaceAdapter, WorkspaceBus,
};
pub use host::{CreateInput, TIMEOUT, UnsupportedHost, WorkspaceHost, WorkspaceOptions};
pub use sse::SseDecoder;
pub use sync::{HistoryEvent, SyncTransport, UnsupportedTransport};

use crate::error::{LocationError, Result};

pub(crate) struct SyncTask {
    pub(crate) cancel: CancellationToken,
    pub(crate) handle: tokio::task::JoinHandle<()>,
}

pub(crate) struct Shared {
    pub(crate) db: Arc<dyn Database>,
    pub(crate) events: Arc<dyn EventStore>,
    pub(crate) adapters: AdapterRegistry,
    pub(crate) bus: WorkspaceBus,
    pub(crate) transport: Arc<dyn SyncTransport>,
    pub(crate) host: Arc<dyn WorkspaceHost>,
    pub(crate) options: WorkspaceOptions,
    /// Insertion-ordered like the baseline `Map`.
    pub(crate) connections: Mutex<Vec<(String, ConnectionState)>>,
    pub(crate) syncs: Mutex<HashMap<String, SyncTask>>,
    pub(crate) tasks: TaskTracker,
    pub(crate) shutdown: CancellationToken,
}

impl Shared {
    /// `setStatus`: emit only on change.
    pub(crate) fn set_status(&self, id: &str, status: ConnectionState) {
        {
            let mut connections = lock(&self.connections);
            match connections.iter_mut().find(|(k, _)| k == id) {
                Some((_, current)) if *current == status => return,
                Some((_, current)) => *current = status,
                None => connections.push((id.to_owned(), status)),
            }
        }
        self.bus.emit(BusEvent {
            directory: Some("global".into()),
            project: None,
            workspace: Some(id.to_owned()),
            payload: json!({
                "type": "workspace.status",
                "properties": { "workspaceID": id, "status": status },
            }),
        });
    }

    pub(crate) fn status_of(&self, id: &str) -> Option<ConnectionState> {
        lock(&self.connections)
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, s)| *s)
    }
}

/// `Workspace.Service`.
#[derive(Clone)]
pub struct WorkspaceService {
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for WorkspaceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceService").finish_non_exhaustive()
    }
}

impl WorkspaceService {
    pub fn new(
        db: Arc<dyn Database>,
        events: Arc<dyn EventStore>,
        adapters: AdapterRegistry,
        bus: WorkspaceBus,
        transport: Arc<dyn SyncTransport>,
        host: Arc<dyn WorkspaceHost>,
        options: WorkspaceOptions,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                db,
                events,
                adapters,
                bus,
                transport,
                host,
                options,
                connections: Mutex::new(Vec::new()),
                syncs: Mutex::new(HashMap::new()),
                tasks: TaskTracker::new(),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    pub fn adapters(&self) -> &AdapterRegistry {
        &self.shared.adapters
    }

    pub fn bus(&self) -> &WorkspaceBus {
        &self.shared.bus
    }

    pub fn options(&self) -> &WorkspaceOptions {
        &self.shared.options
    }

    /// `Workspace.create`.
    pub async fn create(&self, input: CreateInput, context: &AdapterContext) -> Result<Info> {
        let id = WorkspaceID::ascending(input.id.as_deref())
            .map_err(|e| LocationError::Validation(e.to_string()))?
            .into_inner();
        let adapter = self.shared.adapters.get(&input.project_id, &input.kind)?;
        let draft = Info {
            id: id.clone(),
            kind: input.kind.clone(),
            branch: input.branch.clone(),
            name: slug::create(),
            directory: None,
            extra: input.extra.clone(),
            project_id: input.project_id.clone(),
            time_used: 0,
        };
        let config = adapter.configure(draft, context).await?;
        let info = Info {
            id: id.clone(),
            kind: config.kind,
            branch: config.branch,
            name: config.name,
            directory: config.directory,
            extra: config.extra,
            project_id: input.project_id,
            time_used: now_ms(),
        };
        let row = to_row(&info);
        self.shared
            .db
            .transaction(TxOptions::default(), move |sql| {
                Crud::<WorkspaceRow>::insert(sql, &row).map_err(LocationError::from)
            })
            .await?;

        let fail = |source: LocationError| LocationError::CreateFailed {
            workspace_id: id.clone(),
            source: Box::new(source),
        };
        let mut env = CreateEnv::new();
        env.insert(
            "OPENCODE_AUTH_CONTENT".into(),
            Some(self.shared.host.auth_content().await.map_err(fail)?),
        );
        env.insert("OPENCODE_WORKSPACE_ID".into(), Some(config.id.clone()));
        env.insert(
            "OPENCODE_EXPERIMENTAL_WORKSPACES".into(),
            Some("true".into()),
        );
        env.extend(self.shared.host.otel_env());
        adapter
            .create(&info, &env, None, context)
            .await
            .map_err(fail)?;

        // Subscribe before starting sync so the first status change is never missed.
        let mut events = self.shared.bus.subscribe();
        let target = info.id.clone();
        let wait = async move {
            loop {
                match events.recv().await {
                    Ok(event)
                        if event.workspace.as_deref() == Some(target.as_str())
                            && event.payload_type() == Some("workspace.status") =>
                    {
                        let status = event.payload["properties"]["status"].as_str();
                        if matches!(status, Some("error" | "connected")) {
                            return;
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        futures::future::pending::<()>().await
                    }
                }
            }
        };
        let timeout = self.shared.options.timeout;
        let (waited, ()) = tokio::join!(
            tokio::time::timeout(timeout, wait),
            sync::start_sync(&self.shared, &info, context)
        );
        waited.map_err(|_| fail(LocationError::Timeout))?;
        Ok(info)
    }

    /// `Workspace.list`: empty unless experimental workspaces are on, ordered by id.
    pub async fn list(&self, project_id: &str) -> Result<Vec<Info>> {
        if !self.shared.options.experimental_workspaces {
            return Ok(Vec::new());
        }
        let project = project_id.to_owned();
        let rows = self
            .shared
            .db
            .read(move |sql| {
                Crud::<WorkspaceRow>::list(
                    sql,
                    Some("\"project_id\" = ?1"),
                    &[project.into()],
                    None,
                )
            })
            .await?;
        let mut infos: Vec<Info> = rows.iter().map(from_row).collect();
        infos.sort_by(|a, b| locale_compare(&a.id, &b.id));
        Ok(infos)
    }

    /// `Workspace.syncList`: persist adapter-discovered workspaces with unseen names.
    pub async fn sync_list(&self, project_id: &str, context: &AdapterContext) -> Result<()> {
        let mut names: Vec<String> = self
            .list(project_id)
            .await?
            .into_iter()
            .map(|w| w.name)
            .collect();
        let lists =
            futures::future::join_all(self.shared.adapters.registered(project_id).into_iter().map(
                |(kind, adapter)| async move {
                    adapter.list(context).await.unwrap_or_else(|error| {
                        tracing::warn!(r#type = kind, %error, "workspace adapter list failed");
                        Vec::new()
                    })
                },
            ))
            .await;
        for item in lists.into_iter().flatten() {
            if names.contains(&item.name) {
                continue;
            }
            names.push(item.name.clone());
            let info = Info {
                id: WorkspaceID::create().into_inner(),
                kind: item.kind,
                branch: item.branch,
                name: item.name,
                directory: item.directory,
                extra: item.extra,
                project_id: item.project_id,
                time_used: now_ms(),
            };
            let row = to_row(&info);
            self.shared
                .db
                .transaction(TxOptions::default(), move |sql| {
                    Crud::<WorkspaceRow>::insert(sql, &row).map_err(LocationError::from)
                })
                .await?;
            sync::start_sync(&self.shared, &info, context).await;
        }
        Ok(())
    }

    /// `Workspace.get`.
    pub async fn get(&self, id: &str) -> Result<Option<Info>> {
        let id = id.to_owned();
        let row = self
            .shared
            .db
            .read(move |sql| Crud::<WorkspaceRow>::get(sql, &[id.into()]))
            .await?;
        Ok(row.as_ref().map(from_row))
    }

    /// `Workspace.remove`.
    pub async fn remove(&self, id: &str, context: &AdapterContext) -> Result<Option<Info>> {
        let workspace = id.to_owned();
        let sessions: Vec<(String, Option<String>)> = self
            .shared
            .db
            .read(move |sql| {
                sql.query(
                    "SELECT \"id\", \"parent_id\" FROM \"session\" WHERE \"workspace_id\" = ?1",
                    &[workspace.into()],
                )?
                .into_iter()
                .map(|row| Ok((row.get("id")?, row.get_index::<Option<String>>(1)?)))
                .collect::<Result<Vec<_>, opencode_storage_api::StorageError>>()
            })
            .await?;
        let ids: Vec<&String> = sessions.iter().map(|(id, _)| id).collect();
        for (session, parent) in &sessions {
            if parent.as_ref().is_some_and(|p| ids.contains(&p)) {
                continue;
            }
            match self.shared.host.remove_session(session).await {
                Err(error) if error.is_not_found() => {}
                other => other?,
            }
        }
        let Some(info) = self.get(id).await? else {
            return Ok(None);
        };
        sync::stop_sync(&self.shared, id).await;
        let removed = match self.shared.adapters.get(&info.project_id, &info.kind) {
            Ok(adapter) => adapter.remove(&info, context).await,
            Err(error) => Err(error),
        };
        if let Err(error) = removed {
            tracing::error!(r#type = info.kind, %error, "adapter not available when removing workspace");
        }
        let key = id.to_owned();
        self.shared
            .db
            .transaction(TxOptions::default(), move |sql| {
                Crud::<WorkspaceRow>::delete(sql, &[key.into()]).map_err(LocationError::from)
            })
            .await?;
        Ok(Some(info))
    }

    /// `Workspace.status`.
    pub fn status(&self) -> Vec<(String, ConnectionState)> {
        lock(&self.shared.connections).clone()
    }

    /// `Workspace.isSyncing`: a live sync task whose status is not `error`.
    pub fn is_syncing(&self, id: &str) -> bool {
        let live = lock(&self.shared.syncs)
            .get(id)
            .is_some_and(|task| !task.handle.is_finished());
        live && self.shared.status_of(id) != Some(ConnectionState::Error)
    }

    /// `Workspace.startWorkspaceSyncing`: start sync for every workspace of the project, detached.
    pub async fn start_workspace_syncing(
        &self,
        project_id: &str,
        context: &AdapterContext,
    ) -> Result<()> {
        if !self.shared.options.experimental_workspaces {
            return Ok(());
        }
        let project = project_id.to_owned();
        let rows = self
            .shared
            .db
            .read(move |sql| {
                Crud::<WorkspaceRow>::list(
                    sql,
                    Some("\"project_id\" = ?1"),
                    &[project.into()],
                    None,
                )
            })
            .await?;
        for row in rows {
            let shared = self.shared.clone();
            let context = context.clone();
            self.shared.tasks.spawn(async move {
                sync::start_sync(&shared, &from_row(&row), &context).await;
            });
        }
        Ok(())
    }

    /// Stop every sync loop and join every task the service owns.
    pub async fn shutdown(&self) {
        self.shared.shutdown.cancel();
        let tasks: Vec<SyncTask> = lock(&self.shared.syncs).drain().map(|(_, t)| t).collect();
        for task in tasks {
            task.cancel.cancel();
            let _ = task.handle.await;
        }
        self.shared.tasks.close();
        self.shared.tasks.wait().await;
    }
}

pub(crate) fn from_row(row: &WorkspaceRow) -> Info {
    Info {
        id: row.id.clone(),
        kind: row.kind.clone(),
        branch: row.branch.clone(),
        name: row.name.clone(),
        directory: row.directory.clone(),
        extra: row.extra.clone(),
        project_id: row.project_id.clone(),
        time_used: row.time_used,
    }
}

fn to_row(info: &Info) -> WorkspaceRow {
    WorkspaceRow {
        id: info.id.clone(),
        kind: info.kind.clone(),
        name: info.name.clone(),
        branch: info.branch.clone(),
        directory: info.directory.clone(),
        extra: info.extra.clone(),
        project_id: info.project_id.clone(),
        time_used: info.time_used,
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
