//! Purpose: per-Location `LanguageService` implementation: enabled server list from config, client
//! lookup per file with (server, root) dedup, broken set, in-flight spawn dedupe, the feature
//! requests (hover, definition, references, implementation, symbols, call hierarchy), merged
//! diagnostics, `lsp.updated` notifications, restart and joined shutdown.
//! Ports packages/opencode/src/lsp/lsp.ts:96-507.
//! Scope: one Location (InstanceState). Process work is delegated to launch/resolve/client.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/inventory.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! |-- integration-adapters/src/lsp/text.rs
//! `-- integration-api/src/language.rs
//! External:
//! |-- futures::future::Shared
//! `-- tokio::sync::{broadcast, oneshot}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/requests.rs
//! `-- integration-adapters/tests/lsp_service.rs
//! Invariants: at most one client per (server id, root), a key in the broken set is never retried
//! until restart, concurrent lookups for the same key share one spawn, no lock is held across an
//! await, shutdown stops every client and joins every spawn task.
//! Contract tests: integration-adapters/tests/lsp_service.rs

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use opencode_integration_api::language::LspConfig;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::client::{Client, Timeouts};
use super::inventory::{ServerDef, configured};
use super::launch::LaunchEnv;
use super::resolve::resolve;
use super::roots::RootContext;
use super::text::{absolute, contains, match_extension};

type Inflight = Shared<BoxFuture<'static, Option<Arc<Client>>>>;

/// Construction inputs for one Location's language service.
#[derive(Debug, Clone)]
pub struct LspOptions {
    pub config: Option<LspConfig>,
    pub context: RootContext,
    pub launch: LaunchEnv,
    pub timeouts: Timeouts,
}

#[derive(Default)]
struct State {
    clients: Vec<Arc<Client>>,
    broken: HashSet<String>,
    spawning: HashMap<String, Inflight>,
    tasks: Vec<JoinHandle<()>>,
    closed: bool,
}

pub struct LspService {
    pub(super) servers: Vec<ServerDef>,
    pub(super) ctx: RootContext,
    launch: LaunchEnv,
    timeouts: Timeouts,
    state: Arc<Mutex<State>>,
    updated: broadcast::Sender<()>,
}

impl LspService {
    pub fn new(options: LspOptions) -> Self {
        let servers = configured(options.config.as_ref(), options.launch.experimental_ty);
        let ids: Vec<&str> = servers.iter().map(|s| s.id.as_str()).collect();
        tracing::info!(server_ids = %ids.join(", "), "enabled LSP servers");
        Self {
            servers,
            ctx: options.context,
            launch: options.launch,
            timeouts: options.timeouts,
            state: Arc::new(Mutex::new(State::default())),
            updated: broadcast::channel(64).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `lsp.updated` events, one per newly connected client.
    pub fn subscribe_updated(&self) -> broadcast::Receiver<()> {
        self.updated.subscribe()
    }

    pub fn server_ids(&self) -> Vec<String> {
        self.servers.iter().map(|s| s.id.clone()).collect()
    }

    pub(super) fn clients(&self) -> Vec<Arc<Client>> {
        self.lock().clients.clone()
    }

    pub fn broken(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().broken.iter().cloned().collect();
        v.sort();
        v
    }

    fn in_location(&self, file: &Path) -> bool {
        if contains(&self.ctx.directory, file) {
            return true;
        }
        self.ctx.worktree != Path::new("/") && contains(&self.ctx.worktree, file)
    }

    /// lsp.ts:270-277 `hasClients`.
    pub(super) fn has_clients_for(&self, file: &str) -> bool {
        let ext = match_extension(file);
        let path = absolute(&self.ctx.directory, file);
        let broken = self.lock().broken.clone();
        self.servers.iter().filter(|s| s.matches(ext)).any(|s| {
            s.root
                .resolve(&path, &self.ctx)
                .is_some_and(|root| !broken.contains(&key(&root, &s.id)))
        })
    }

    /// lsp.ts:200-262 `getClients`.
    pub(super) async fn clients_for(&self, file: &str) -> Vec<Arc<Client>> {
        let path = absolute(&self.ctx.directory, file);
        if !self.in_location(&path) {
            return Vec::new();
        }
        let ext = match_extension(file).to_string();
        let mut result = Vec::new();
        for server in self.servers.iter().filter(|s| s.matches(&ext)) {
            let Some(root) = server.root.resolve(&path, &self.ctx) else {
                continue;
            };
            let k = key(&root, &server.id);
            let inflight = {
                let mut state = self.lock();
                if state.closed || state.broken.contains(&k) {
                    continue;
                }
                if let Some(found) = state
                    .clients
                    .iter()
                    .find(|c| c.root == root && c.server_id == server.id)
                {
                    result.push(found.clone());
                    continue;
                }
                match state.spawning.get(&k) {
                    Some(existing) => existing.clone(),
                    None => {
                        let fut =
                            self.schedule(server.clone(), root.clone(), k.clone(), &mut state);
                        state.spawning.insert(k.clone(), fut.clone());
                        fut
                    }
                }
            };
            if let Some(client) = inflight.await
                && !result.iter().any(|c| Arc::ptr_eq(c, &client))
            {
                result.push(client);
            }
        }
        result
    }

    /// lsp.ts:211-239 `schedule`, run on an owned task so a cancelled caller cannot strand it.
    fn schedule(
        &self,
        server: ServerDef,
        root: std::path::PathBuf,
        k: String,
        state: &mut State,
    ) -> Inflight {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let shared = self.state.clone();
        let launch = self.launch.clone();
        let ctx = self.ctx.clone();
        let timeouts = self.timeouts;
        let updated = self.updated.clone();
        let task = tokio::spawn(async move {
            let client = connect(&server, &root, &ctx, &launch, timeouts).await;
            // (result for waiters, duplicate client to stop)
            let (result, stop) = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.spawning.remove(&k);
                match client {
                    None => {
                        state.broken.insert(k);
                        (None, None)
                    }
                    Some(c) if state.closed => (None, Some(c)),
                    Some(c) => match state
                        .clients
                        .iter()
                        .find(|x| x.root == root && x.server_id == server.id)
                    {
                        // lsp.ts:230-234: an existing (server, root) client wins, stop the new one.
                        Some(existing) => (Some(existing.clone()), Some(c)),
                        None => {
                            state.clients.push(c.clone());
                            let _ = updated.send(());
                            (Some(c), None)
                        }
                    },
                }
            };
            if let Some(dup) = stop {
                dup.shutdown().await;
            }
            let _ = tx.send(result);
        });
        state.tasks.retain(|t| !t.is_finished());
        state.tasks.push(task);
        rx.map(|r| r.ok().flatten()).boxed().shared()
    }

    /// Stop every client, forget the broken set and allow fresh spawns (manual restart).
    pub async fn restart(&self) {
        self.stop_all(false).await;
    }

    async fn stop_all(&self, close: bool) {
        let (clients, tasks) = {
            let mut state = self.lock();
            state.closed = true;
            (
                std::mem::take(&mut state.clients),
                std::mem::take(&mut state.tasks),
            )
        };
        for task in tasks {
            let _ = task.await;
        }
        let late = std::mem::take(&mut self.lock().clients);
        futures::future::join_all(clients.iter().chain(late.iter()).map(|c| c.shutdown())).await;
        let mut state = self.lock();
        state.spawning.clear();
        if !close {
            state.broken.clear();
            state.closed = false;
        }
    }

    pub(super) async fn shutdown_all(&self) {
        self.stop_all(true).await;
    }
}

fn key(root: &Path, id: &str) -> String {
    format!("{}{id}", root.display())
}

/// Resolve, spawn and initialize. `None` marks the key broken (baseline swallows the cause).
async fn connect(
    server: &ServerDef,
    root: &Path,
    ctx: &RootContext,
    launch: &LaunchEnv,
    timeouts: Timeouts,
) -> Option<Arc<Client>> {
    let spec = match resolve(server, root, ctx, launch).await {
        Ok(Some(spec)) => spec,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(server = %server.id, error = %err, "lsp server unavailable");
            return None;
        }
    };
    let handle = launch
        .spawn(&spec, root)
        .map_err(|err| tracing::warn!(server = %server.id, error = %err, "lsp spawn failed"))
        .ok()?;
    Client::create(&server.id, handle, root, &ctx.directory, timeouts)
        .await
        .map_err(|err| tracing::warn!(server = %server.id, error = %err, "lsp initialize failed"))
        .ok()
}
