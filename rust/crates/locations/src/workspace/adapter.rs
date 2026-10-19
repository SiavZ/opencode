//! Purpose: workspace adapter contract and registry (ports of packages/opencode/src/control-plane/
//! types.ts, adapters/index.ts and workspace-adapter-runtime.ts), plus the workspace value types and
//! the process-local workspace bus (`GlobalBus` events tagged with a workspace).
//! Scope: traits with typed `Unsupported` defaults, adapter registry, wire structs.
//! Uses:
//! rust/crates/
//! `-- locations/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- broadcast::Receiver
//! |-- broadcast::Sender
//! |-- broadcast::channel
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! `-- tokio::sync::broadcast
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! `-- locations/src/workspace/worktree.rs
//! Invariants: per-project adapters override built-ins with the same type; unknown types fail with
//! `Unknown workspace adapter: <type>`; every adapter method an adapter does not implement fails
//! with `LocationError::Unsupported` naming the adapter and method, except `list` (empty, like the
//! optional `list?`). `Info` serializes with explicit nulls in `fromRow` field order.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tokio::sync::broadcast;

use crate::error::{LocationError, Result};

/// `Workspace.Info` (`WorkspaceInfo & { timeUsed }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub branch: Option<String>,
    pub name: String,
    pub directory: Option<String>,
    pub extra: Option<Json>,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "timeUsed")]
    pub time_used: i64,
}

/// `WorkspaceListedInfo` (adapter `list` result, no id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedInfo {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub extra: Option<Json>,
    #[serde(rename = "projectID")]
    pub project_id: String,
}

/// `Target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local {
        directory: String,
    },
    Remote {
        url: String,
        headers: Vec<(String, String)>,
    },
}

/// Instance context handed to adapters (`InstanceRef`/`WorkspaceRef`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterContext {
    pub project: Option<ProjectContext>,
    pub workspace_id: Option<String>,
}

/// The parts of `InstanceContext` adapters read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    pub id: String,
    /// Project worktree (main checkout).
    pub worktree: String,
    /// `project.vcs === "git"`.
    pub git: bool,
}

/// Environment passed to `create` (`undefined` values are `None`).
pub type CreateEnv = BTreeMap<String, Option<String>>;

/// `WorkspaceAdapter`. Methods an adapter does not implement fail with a typed `Unsupported`.
#[async_trait]
pub trait WorkspaceAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    async fn configure(&self, info: Info, context: &AdapterContext) -> Result<Info> {
        let _ = (info, context);
        Err(self.unsupported("configure"))
    }

    async fn create(
        &self,
        info: &Info,
        env: &CreateEnv,
        from: Option<&Info>,
        context: &AdapterContext,
    ) -> Result<()> {
        let _ = (info, env, from, context);
        Err(self.unsupported("create"))
    }

    /// Optional in the baseline: absent means no discovered workspaces.
    async fn list(&self, context: &AdapterContext) -> Result<Vec<ListedInfo>> {
        let _ = context;
        Ok(Vec::new())
    }

    async fn remove(&self, info: &Info, context: &AdapterContext) -> Result<()> {
        let _ = (info, context);
        Err(self.unsupported("remove"))
    }

    async fn target(&self, info: &Info, context: &AdapterContext) -> Result<Target> {
        let _ = (info, context);
        Err(self.unsupported("target"))
    }

    fn unsupported(&self, method: &str) -> LocationError {
        LocationError::unsupported(
            format!("workspace adapter {}.{method}", self.name()),
            "this adapter needs a later phase (remote hosts, plugin bridge)",
        )
    }
}

type Adapters = BTreeMap<String, Arc<dyn WorkspaceAdapter>>;

/// `adapters/index.ts`: built-ins plus per-project registrations.
#[derive(Clone, Default)]
pub struct AdapterRegistry {
    builtin: Arc<RwLock<Adapters>>,
    custom: Arc<RwLock<HashMap<String, Adapters>>>,
}

impl std::fmt::Debug for AdapterRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterRegistry").finish_non_exhaustive()
    }
}

/// `WorkspaceAdapterEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub description: String,
}

impl AdapterRegistry {
    /// Register a built-in (`BUILTIN[type]`).
    pub fn builtin(&self, kind: impl Into<String>, adapter: Arc<dyn WorkspaceAdapter>) {
        write(&self.builtin).insert(kind.into(), adapter);
    }

    /// `registerAdapter(projectID, type, adapter)`. Use `global` for every project.
    pub fn register(
        &self,
        project_id: &str,
        kind: impl Into<String>,
        adapter: Arc<dyn WorkspaceAdapter>,
    ) {
        write(&self.custom)
            .entry(project_id.to_owned())
            .or_default()
            .insert(kind.into(), adapter);
    }

    /// `getAdapter`.
    pub fn get(&self, project_id: &str, kind: &str) -> Result<Arc<dyn WorkspaceAdapter>> {
        if let Some(adapter) = read(&self.custom).get(project_id).and_then(|m| m.get(kind)) {
            return Ok(adapter.clone());
        }
        read(&self.builtin)
            .get(kind)
            .cloned()
            .ok_or_else(|| LocationError::Adapter {
                message: format!("Unknown workspace adapter: {kind}"),
            })
    }

    /// `registeredAdapters`: built-ins in insertion-independent key order, overridden by custom.
    pub fn registered(&self, project_id: &str) -> Vec<(String, Arc<dyn WorkspaceAdapter>)> {
        let mut adapters = read(&self.builtin).clone();
        if let Some(custom) = read(&self.custom).get(project_id) {
            adapters.extend(custom.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        adapters.into_iter().collect()
    }

    /// `listAdapters`.
    pub fn entries(&self, project_id: &str) -> Vec<AdapterEntry> {
        self.registered(project_id)
            .into_iter()
            .map(|(kind, adapter)| AdapterEntry {
                kind,
                name: adapter.name().to_owned(),
                description: adapter.description().to_owned(),
            })
            .collect()
    }
}

/// One `GlobalBus` "event" emission.
#[derive(Debug, Clone, PartialEq)]
pub struct BusEvent {
    pub directory: Option<String>,
    pub project: Option<String>,
    pub workspace: Option<String>,
    /// `{ type, properties }`.
    pub payload: Json,
}

impl BusEvent {
    pub fn payload_type(&self) -> Option<&str> {
        self.payload.get("type").and_then(Json::as_str)
    }
}

/// Process-local bus. Lagging receivers skip events (bounded, overflow drops the oldest).
#[derive(Debug, Clone)]
pub struct WorkspaceBus {
    sender: broadcast::Sender<BusEvent>,
}

impl Default for WorkspaceBus {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(1024).0,
        }
    }
}

impl WorkspaceBus {
    pub fn emit(&self, event: BusEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
