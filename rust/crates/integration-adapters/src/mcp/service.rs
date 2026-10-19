//! Purpose: per-Location MCP service state, port of the `MCP.layer` state machine (baseline
//! index.ts:442-788): lazy initial connect of configured servers, status union, client/defs/
//! instructions maps, notification watchers (tools/list_changed refresh, connection close),
//! and the helpers behind add/connect/disconnect and catalog queries.
//! Scope: one service instance per Location; trait impl in service_api.rs, auth in service_auth.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! |-- integration-adapters/src/mcp/browser.rs
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/src/mcp/oauth_callback.rs
//! |-- integration-adapters/src/mcp/stdio.rs
//! |-- integration-adapters/src/mcp/transport.rs
//! |-- integration-api/src/error.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- catalog::keyed
//! |-- futures::future::join_all
//! |-- nix::sys::signal::Signal::SIGTERM
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- tokio::select
//! |-- tokio::spawn
//! |-- tokio::sync::OnceCell
//! |-- tokio::sync::OnceCell::new
//! |-- tokio::task::JoinHandle
//! |-- tracing::error
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/service_api.rs
//! `-- integration-adapters/src/mcp/service_auth.rs
//! Invariants: the state lock is never held across an await; a watcher only mutates state while
//! its client is still the stored client for that name; shutdown SIGTERMs stdio descendants
//! then closes every client and joins watchers.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs, integration-adapters/tests/mcp_http.rs

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::future::join_all;
use opencode_integration_api::error::IntegrationError;
use opencode_integration_api::mcp::{McpEvent, ServerConfig, Status, ToolDef, ToolsChangedProps};
use serde_json::Value;

use super::auth_store::McpAuthStore;
use super::browser::{BrowserOpener, SystemBrowser};
use super::catalog;
use super::client::McpClient;
use super::connect::{ConnectEnv, PendingAuth, create};
use super::jsonrpc::RpcError;
use super::oauth_callback::OAuthCallback;

/// `TuiEvent.ToastShow` payload the baseline publishes for auth-required servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub title: String,
    pub message: String,
    pub variant: &'static str,
    pub duration: u64,
}

pub trait McpEvents: Send + Sync {
    fn publish(&self, event: McpEvent);
    fn toast(&self, _toast: Toast) {}
}

pub struct NoopEvents;
impl McpEvents for NoopEvents {
    fn publish(&self, _event: McpEvent) {}
}

pub struct McpOptions {
    /// Location directory: stdio cwd base and the single `roots/list` root.
    pub directory: PathBuf,
    /// Data dir holding `mcp-auth.json` (inject a temp dir in tests).
    pub data_dir: PathBuf,
    /// Typed `config.mcp` entries (untyped entries are dropped by the config layer).
    pub config: BTreeMap<String, ServerConfig>,
    /// `experimental.mcp_timeout`.
    pub default_timeout_ms: Option<u64>,
    pub events: Arc<dyn McpEvents>,
    pub browser: Arc<dyn BrowserOpener>,
    pub callback: OAuthCallback,
}

impl McpOptions {
    pub fn new(
        directory: PathBuf,
        data_dir: PathBuf,
        config: BTreeMap<String, ServerConfig>,
    ) -> Self {
        Self {
            directory,
            data_dir,
            config,
            default_timeout_ms: None,
            events: Arc::new(NoopEvents),
            browser: Arc::new(SystemBrowser),
            callback: OAuthCallback::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    pub config: BTreeMap<String, ServerConfig>,
    pub status: BTreeMap<String, Status>,
    pub clients: BTreeMap<String, Arc<McpClient>>,
    pub defs: BTreeMap<String, Vec<ToolDef>>,
    pub instructions: BTreeMap<String, String>,
    pub watchers: BTreeMap<String, tokio::task::JoinHandle<()>>,
    pub pending: HashMap<String, PendingAuth>,
}

pub(super) struct Inner {
    pub directory: PathBuf,
    pub static_config: BTreeMap<String, ServerConfig>,
    pub default_timeout_ms: Option<u64>,
    pub store: McpAuthStore,
    pub http: reqwest::Client,
    pub events: Arc<dyn McpEvents>,
    pub browser: Arc<dyn BrowserOpener>,
    pub callback: OAuthCallback,
    pub state: Mutex<State>,
    init: tokio::sync::OnceCell<()>,
}

#[derive(Clone)]
pub struct McpServiceImpl(pub(super) Arc<Inner>);

pub(super) fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn rpc_to_integration(name: &str, e: RpcError) -> IntegrationError {
    match e {
        RpcError::Mcp { code, .. } if code == super::jsonrpc::REQUEST_TIMEOUT => {
            IntegrationError::Timeout { ms: 0 }
        }
        RpcError::Cancelled => IntegrationError::Cancelled,
        RpcError::Mcp { code, message, .. } if code == super::jsonrpc::CONNECTION_CLOSED => {
            IntegrationError::Closed(message)
        }
        RpcError::Transport(super::transport::TransportError::Closed) => {
            IntegrationError::Closed("Not connected".into())
        }
        other => IntegrationError::Failed {
            name: name.into(),
            message: other.to_string(),
        },
    }
}

impl McpServiceImpl {
    pub fn new(opts: McpOptions) -> Self {
        Self(Arc::new(Inner {
            directory: opts.directory,
            static_config: opts.config,
            default_timeout_ms: opts.default_timeout_ms,
            store: McpAuthStore::new(opts.data_dir),
            http: reqwest::Client::new(),
            events: opts.events,
            browser: opts.browser,
            callback: opts.callback,
            state: Mutex::default(),
            init: tokio::sync::OnceCell::new(),
        }))
    }

    pub fn auth_store(&self) -> &McpAuthStore {
        &self.0.store
    }

    pub(super) fn env(&self) -> ConnectEnv<'_> {
        ConnectEnv {
            directory: &self.0.directory,
            store: &self.0.store,
            http: &self.0.http,
        }
    }

    /// Lazily connect all enabled configured servers concurrently (InstanceState.make).
    pub(super) async fn ready(&self) {
        self.0.init.get_or_init(|| self.initialize()).await;
    }

    async fn initialize(&self) {
        let entries: Vec<_> = self.0.static_config.clone().into_iter().collect();
        let results = join_all(entries.into_iter().map(|(key, cfg)| async move {
            if !cfg.enabled() {
                return (key, None, cfg);
            }
            let r = create(&self.env(), &key, &cfg).await;
            (key, Some(r), cfg)
        }))
        .await;
        for (key, result, cfg) in results {
            let Some(result) = result else {
                lock(&self.0.state).status.insert(key, Status::Disabled);
                continue;
            };
            self.apply(&key, result, cfg.timeout_ms(), false).await;
        }
    }

    /// Store a `create` outcome (`createAndStore` / `storeClient`). Returns the new status.
    pub(super) async fn apply(
        &self,
        name: &str,
        mut result: super::connect::CreateResult,
        timeout: Option<u64>,
        close_previous: bool,
    ) -> Status {
        if let Some(t) = result.toast.take() {
            self.0.events.toast(Toast {
                title: "MCP Authentication Required".into(),
                message: t,
                variant: "warning",
                duration: 8000,
            });
        }
        let Some(c) = result.connected.take() else {
            let (prev, watcher) = {
                let mut s = lock(&self.0.state);
                s.status.insert(name.into(), result.status.clone());
                if let Some(p) = result.pending.take() {
                    s.pending.insert(name.into(), p);
                }
                s.defs.remove(name);
                s.instructions.remove(name);
                (s.clients.remove(name), s.watchers.remove(name))
            };
            if close_previous && let Some(prev) = prev {
                prev.close().await;
            }
            if let Some(w) = watcher {
                w.abort();
            }
            return result.status;
        };
        self.store_client(name, c.client, c.defs, c.instructions, timeout)
            .await
    }

    pub(super) async fn store_client(
        &self,
        name: &str,
        client: Arc<McpClient>,
        defs: Vec<ToolDef>,
        instructions: Option<String>,
        timeout: Option<u64>,
    ) -> Status {
        let watcher = self.watch(name, client.clone(), timeout);
        let (prev, old_watcher) = {
            let mut s = lock(&self.0.state);
            s.status.insert(name.into(), Status::Connected);
            let prev = s.clients.insert(name.into(), client);
            s.defs.insert(name.into(), defs);
            match instructions.filter(|i| !i.is_empty()) {
                Some(i) => s.instructions.insert(name.into(), i),
                None => s.instructions.remove(name),
            };
            (prev, s.watchers.insert(name.into(), watcher))
        };
        if let Some(prev) = prev {
            prev.close().await;
        }
        if let Some(w) = old_watcher {
            w.abort();
            let _ = w.await;
        }
        Status::Connected
    }

    /// Baseline `watch`: connection close and tools/list_changed handling.
    fn watch(
        &self,
        name: &str,
        client: Arc<McpClient>,
        timeout: Option<u64>,
    ) -> tokio::task::JoinHandle<()> {
        let svc = self.clone();
        let name = name.to_string();
        let mut changed = client.tools_changed();
        let mut closed = client.closed();
        let tools = client.has_capability("tools");
        tokio::spawn(async move {
            let current = |svc: &McpServiceImpl| {
                let s = lock(&svc.0.state);
                s.clients
                    .get(&name)
                    .is_some_and(|c| Arc::ptr_eq(c, &client))
                    && s.status.get(&name) == Some(&Status::Connected)
            };
            loop {
                tokio::select! {
                    r = closed.changed() => {
                        if r.is_err() || closed.borrow().is_some() {
                            let removed = {
                                let mut s = lock(&svc.0.state);
                                let mine = s.clients.get(&name).is_some_and(|c| Arc::ptr_eq(c, &client));
                                if mine {
                                    s.clients.remove(&name);
                                    s.defs.remove(&name);
                                    s.instructions.remove(&name);
                                    s.status.insert(name.clone(), Status::Failed { error: "Connection closed".into() });
                                }
                                mine
                            };
                            if removed {
                                tracing::warn!(server = %name, "MCP connection closed");
                                svc.tools_changed(&name);
                            }
                            return;
                        }
                    }
                    r = changed.changed(), if tools => {
                        if r.is_err() || !current(&svc) {
                            continue;
                        }
                        let Some(listed) = catalog::defs(&client, timeout).await else { continue };
                        if !current(&svc) {
                            continue;
                        }
                        lock(&svc.0.state).defs.insert(name.clone(), listed);
                        svc.tools_changed(&name);
                    }
                }
            }
        })
    }

    pub(super) fn tools_changed(&self, name: &str) {
        self.0.events.publish(McpEvent::ToolsChanged {
            properties: ToolsChangedProps {
                server: name.into(),
            },
        });
    }

    pub(super) fn config_for(&self, name: &str) -> Option<ServerConfig> {
        let runtime = lock(&self.0.state).config.get(name).cloned();
        runtime.or_else(|| self.0.static_config.get(name).cloned())
    }

    pub(super) fn require_config(&self, name: &str) -> Result<ServerConfig, IntegrationError> {
        self.config_for(name)
            .ok_or_else(|| IntegrationError::NotFound {
                kind: "MCP server",
                name: name.into(),
            })
    }

    /// Baseline `requestTimeout`: runtime config, then static config, then experimental default.
    pub(super) fn request_timeout(&self, name: &str) -> Option<u64> {
        let runtime = lock(&self.0.state)
            .config
            .get(name)
            .and_then(ServerConfig::timeout_ms);
        runtime
            .or_else(|| {
                self.0
                    .static_config
                    .get(name)
                    .and_then(ServerConfig::timeout_ms)
            })
            .or(self.0.default_timeout_ms)
    }

    pub(super) async fn create_and_store(&self, name: &str, config: ServerConfig) -> Status {
        let result = create(&self.env(), name, &config).await;
        self.apply(name, result, config.timeout_ms(), true).await
    }

    pub(super) fn connected_clients(&self, only: Option<&str>) -> Vec<(String, Arc<McpClient>)> {
        let s = lock(&self.0.state);
        s.clients
            .iter()
            .filter(|(n, _)| {
                s.status.get(*n) == Some(&Status::Connected) && only.is_none_or(|o| o == *n)
            })
            .map(|(n, c)| (n.clone(), c.clone()))
            .collect()
    }

    pub(super) async fn collect<F, Fut>(
        &self,
        only: Option<&str>,
        label: &str,
        key: Option<&str>,
        list: F,
    ) -> BTreeMap<String, Value>
    where
        F: Fn(Arc<McpClient>, Option<u64>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Value>, RpcError>>,
    {
        self.ready().await;
        let clients = self.connected_clients(only);
        let fetched = join_all(clients.into_iter().map(|(name, client)| {
            let fut = list(client, self.request_timeout(&name));
            async move { (name, fut.await) }
        }))
        .await;
        fetched
            .into_iter()
            .flat_map(|(name, r)| match r {
                Ok(items) => catalog::keyed(&name, items, key),
                Err(e) => {
                    tracing::warn!(client = %name, error = %e, "failed to get {label}");
                    BTreeMap::new()
                }
            })
            .collect()
    }

    pub(super) async fn with_client(
        &self,
        name: &str,
        method: &str,
        params: Value,
    ) -> Option<Value> {
        self.ready().await;
        let client = lock(&self.0.state).clients.get(name).cloned();
        let Some(client) = client else {
            tracing::warn!(client = name, "client not found for {method}");
            return None;
        };
        match client
            .request(method, Some(params), self.request_timeout(name))
            .await
        {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(client = name, error = %e, "failed to {method}");
                None
            }
        }
    }

    /// Finalizer: SIGTERM stdio descendants, close clients, join watchers, drop pending flows.
    pub async fn shutdown(&self) {
        let (clients, watchers) = {
            let mut s = lock(&self.0.state);
            s.defs.clear();
            s.instructions.clear();
            s.pending.clear();
            (
                std::mem::take(&mut s.clients),
                std::mem::take(&mut s.watchers),
            )
        };
        join_all(clients.into_values().map(|c| async move {
            if let Some(pid) = c.pid() {
                for d in super::stdio::descendants(pid).await {
                    super::stdio::signal(d, nix::sys::signal::Signal::SIGTERM);
                }
            }
            c.close().await;
        }))
        .await;
        for w in watchers.into_values() {
            w.abort();
            let _ = w.await;
        }
    }
}

pub(super) fn enable(config: ServerConfig) -> ServerConfig {
    match config {
        ServerConfig::Local(mut l) => {
            l.enabled = Some(true);
            ServerConfig::Local(l)
        }
        ServerConfig::Remote(mut r) => {
            r.enabled = Some(true);
            ServerConfig::Remote(r)
        }
    }
}
