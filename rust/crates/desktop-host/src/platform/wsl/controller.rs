//! Purpose: WSL servers controller (`createWslServersController`, `wsl/servers.ts:62-436`): state +
//! subscribers, persisted server list, per-server start generations that dispose stale sidecars,
//! exclusive cancellable jobs, parallel addable-distro probing and background opencode checks.
//! Scope: orchestration over `WslRunner` (commands), `SpawnSidecar` (server launch) and
//! `WslServerStore` (persistence). Spawned tasks are tracked and joined by `shutdown`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller/jobs.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! |-- desktop-host/src/platform/wsl/policy.rs
//! |-- desktop-host/src/platform/wsl/runtime.rs
//! `-- desktop-host/src/platform/wsl/sidecar.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- tokio::task::JoinSet
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller/jobs.rs
//! |-- desktop-host/tests/wsl_servers.rs
//! `-- desktop-host/tests/wsl_support/mod.rs
//! Invariants: a start whose generation is no longer current never publishes `ready`/`failed` and
//! stops the sidecar it produced; removing a server drops late opencode checks; a new job cancels the
//! previous one and only the current job clears `job`; no lock is held across an await.
//! Contract tests: desktop-host/tests/wsl_servers.rs

mod jobs;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::policy::{
    clear_wsl_distro_state, wsl_server_id_for_distro, wsl_server_ids_to_start_on_initialize,
};
pub use super::policy::{normalize_persisted_servers, opencode_check};
use super::runtime::{self, RunOptions, WslError, WslRunner};
pub use super::sidecar::{RunningSidecar, SidecarHandle, SpawnSidecar};
use super::{
    WslJob, WslOpencodeCheck, WslServerConfig, WslServerItem, WslServerRuntime, WslServersEvent,
    WslServersState, messages,
};

/// Persisted WSL server list (electron-store key `WSL_SERVERS_KEY`, owned by persistence).
pub trait WslServerStore: Send + Sync {
    fn read(&self) -> Vec<WslServerConfig>;
    fn write(&self, servers: &[WslServerConfig]);
}

/// In-memory store, also the reference behavior for adapters.
#[derive(Debug, Default)]
pub struct MemoryStore(pub Mutex<Vec<WslServerConfig>>);

impl WslServerStore for MemoryStore {
    fn read(&self) -> Vec<WslServerConfig> {
        self.0.lock().clone()
    }
    fn write(&self, servers: &[WslServerConfig]) {
        *self.0.lock() = servers.to_vec();
    }
}

type Listener = Arc<dyn Fn(&WslServersEvent) + Send + Sync>;

#[derive(Default)]
struct Inner {
    state: WslServersState,
    sidecars: HashMap<String, RunningSidecar>,
    attempts: HashMap<String, u64>,
    job: Option<(u64, CancellationToken)>,
    next_job: u64,
}

struct Shared {
    app_version: String,
    runner: Arc<dyn WslRunner>,
    spawn: SpawnSidecar,
    store: Arc<dyn WslServerStore>,
    inner: Mutex<Inner>,
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_listener: Mutex<u64>,
    tasks: Mutex<JoinSet<()>>,
    shutdown: CancellationToken,
}

/// Cloneable controller handle.
#[derive(Clone)]
pub struct WslServersController(Arc<Shared>);

impl std::fmt::Debug for WslServersController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WslServersController")
            .finish_non_exhaustive()
    }
}

impl WslServersController {
    pub fn new(
        app_version: &str,
        runner: Arc<dyn WslRunner>,
        spawn: SpawnSidecar,
        store: Arc<dyn WslServerStore>,
    ) -> Self {
        Self(Arc::new(Shared {
            app_version: app_version.to_string(),
            runner,
            spawn,
            store,
            inner: Mutex::new(Inner::default()),
            listeners: Mutex::new(Vec::new()),
            next_listener: Mutex::new(0),
            tasks: Mutex::new(JoinSet::new()),
            shutdown: CancellationToken::new(),
        }))
    }

    pub fn state(&self) -> WslServersState {
        self.0.inner.lock().state.clone()
    }

    /// Returns a subscription id for [`Self::unsubscribe`].
    pub fn subscribe(&self, listener: impl Fn(&WslServersEvent) + Send + Sync + 'static) -> u64 {
        let mut next = self.0.next_listener.lock();
        *next += 1;
        self.0.listeners.lock().push((*next, Arc::new(listener)));
        *next
    }

    pub fn unsubscribe(&self, id: u64) {
        self.0.listeners.lock().retain(|(key, _)| *key != id);
    }

    fn update(&self, change: impl FnOnce(&mut WslServersState)) {
        let snapshot = {
            let mut inner = self.0.inner.lock();
            change(&mut inner.state);
            inner.state.clone()
        };
        let listeners: Vec<Listener> = self
            .0
            .listeners
            .lock()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        let event = WslServersEvent::State { state: snapshot };
        listeners.iter().for_each(|listener| listener(&event));
    }

    fn set_runtime(&self, id: &str, runtime: WslServerRuntime) {
        self.update(|state| {
            state
                .servers
                .iter_mut()
                .filter(|item| item.config.id == id)
                .for_each(|item| item.runtime = runtime.clone());
        });
    }

    fn spawn_task(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        self.0.tasks.lock().spawn(future);
    }

    fn has_server(&self, id: &str, distro: &str) -> bool {
        self.0
            .inner
            .lock()
            .state
            .servers
            .iter()
            .any(|item| item.config.id == id && item.config.distro == distro)
    }

    async fn check_opencode(
        &self,
        distro: &str,
        options: &RunOptions,
    ) -> Result<WslOpencodeCheck, WslError> {
        let runner = self.0.runner.as_ref();
        let resolved = runtime::resolve_opencode(runner, distro, options).await?;
        let version = match &resolved {
            Some(path) => runtime::read_command_version(runner, path, distro, options).await?,
            None => None,
        };
        Ok(opencode_check(
            distro,
            resolved,
            version,
            &self.0.app_version,
        ))
    }

    fn refresh_check_background(&self, id: String, distro: String) {
        let this = self.clone();
        self.spawn_task(async move {
            match this.check_opencode(&distro, &RunOptions::default()).await {
                Ok(check) if this.has_server(&id, &distro) => {
                    this.update(|state| state.opencode_checks.insert(distro.clone(), check));
                }
                Ok(_) => {}
                Err(error) => tracing::error!(id, distro, %error, "wsl opencode check failed"),
            }
        });
    }

    fn next_attempt(&self, id: &str) -> u64 {
        let mut inner = self.0.inner.lock();
        let next = inner.attempts.get(id).copied().unwrap_or(0) + 1;
        inner.attempts.insert(id.to_string(), next);
        next
    }

    fn is_current(&self, id: &str, attempt: u64) -> bool {
        let inner = self.0.inner.lock();
        inner.attempts.get(id) == Some(&attempt)
            && inner.state.servers.iter().any(|item| item.config.id == id)
    }

    async fn stop_internal(&self, id: &str) {
        let existing = self.0.inner.lock().sidecars.remove(id);
        if let Some(existing) = existing {
            existing.handle.stop().await;
        }
    }

    /// `startServer`: bump the generation, stop any running sidecar, spawn, and publish only if the
    /// generation is still current; stale sidecars are stopped.
    pub async fn start_server(&self, id: &str) {
        let Some(distro) = self
            .0
            .inner
            .lock()
            .state
            .servers
            .iter()
            .find(|item| item.config.id == id)
            .map(|item| item.config.distro.clone())
        else {
            return;
        };
        let attempt = self.next_attempt(id);
        self.stop_internal(id).await;
        if !self.is_current(id, attempt) {
            return;
        }
        self.set_runtime(id, WslServerRuntime::Starting);
        tracing::info!(id, distro, "wsl sidecar starting");
        match (self.0.spawn)(distro.clone(), self.0.shutdown.child_token()).await {
            Ok(sidecar) => {
                if !self.is_current(id, attempt) {
                    sidecar.handle.stop().await;
                    return;
                }
                self.0
                    .inner
                    .lock()
                    .sidecars
                    .insert(id.to_string(), sidecar.clone());
                self.set_runtime(
                    id,
                    WslServerRuntime::Ready {
                        url: sidecar.ready.url.clone(),
                        username: sidecar.ready.username.clone(),
                        password: sidecar.ready.password.clone(),
                    },
                );
                self.watch_exit(id.to_string(), distro.clone(), sidecar);
                self.refresh_check_background(id.to_string(), distro.clone());
                tracing::info!(id, distro, "wsl sidecar ready");
            }
            Err(message) => {
                if !self.is_current(id, attempt) {
                    return;
                }
                self.set_runtime(
                    id,
                    WslServerRuntime::Failed {
                        message: message.clone(),
                    },
                );
                tracing::error!(id, distro, message, "wsl sidecar failed to start");
            }
        }
    }

    fn watch_exit(&self, id: String, distro: String, sidecar: RunningSidecar) {
        let this = self.clone();
        self.spawn_task(async move {
            let exit = sidecar.handle.exited().await;
            let current = {
                let mut inner = this.0.inner.lock();
                let same = inner
                    .sidecars
                    .get(&id)
                    .is_some_and(|running| Arc::ptr_eq(&running.handle, &sidecar.handle));
                if same {
                    inner.sidecars.remove(&id);
                }
                same
            };
            if !current {
                return;
            }
            let message = messages::server_exited(&exit.code_text(), &exit.signal_text());
            this.set_runtime(&id, WslServerRuntime::Failed { message });
            tracing::error!(id, distro, ?exit, "wsl sidecar exited");
        });
    }

    async fn run_job<T>(
        &self,
        job: WslJob,
        work: impl AsyncFnOnce(CancellationToken) -> Result<T, WslError>,
    ) -> Result<Option<T>, WslError> {
        let (job_id, cancel) = {
            let mut inner = self.0.inner.lock();
            if let Some((_, previous)) = inner.job.take() {
                previous.cancel();
            }
            inner.next_job += 1;
            let cancel = self.0.shutdown.child_token();
            inner.job = Some((inner.next_job, cancel.clone()));
            (inner.next_job, cancel)
        };
        self.update(|state| state.job = Some(job));
        let result = work(cancel).await;
        let current = {
            let mut inner = self.0.inner.lock();
            let current = inner.job.as_ref().is_some_and(|(id, _)| *id == job_id);
            if current {
                inner.job = None;
            }
            current
        };
        if current {
            self.update(|state| state.job = None);
        }
        match result {
            Ok(value) => Ok(Some(value)),
            Err(WslError::Aborted) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `initialize`: load persisted servers, check opencode for each in the background, start all.
    pub fn initialize(&self) {
        let persisted = self.0.store.read();
        self.update(|state| {
            state.servers = persisted
                .into_iter()
                .map(|config| {
                    let runtime = state
                        .servers
                        .iter()
                        .find(|item| item.config.id == config.id)
                        .map_or(WslServerRuntime::Stopped, |item| item.runtime.clone());
                    WslServerItem { config, runtime }
                })
                .collect();
        });
        let servers: Vec<WslServerConfig> = self
            .state()
            .servers
            .into_iter()
            .map(|item| item.config)
            .collect();
        servers.iter().for_each(|config| {
            self.refresh_check_background(config.id.clone(), config.distro.clone())
        });
        for id in wsl_server_ids_to_start_on_initialize(&servers) {
            let this = self.clone();
            self.spawn_task(async move { this.start_server(&id).await });
        }
    }

    /// `addServer`: persist, publish as `starting`, start in the background.
    pub fn add_server(&self, distro: &str) -> Result<WslServerConfig, WslError> {
        let id = wsl_server_id_for_distro(distro);
        if self.state().servers.iter().any(|item| item.config.id == id) {
            return Err(WslError::Message(messages::already_added(distro)));
        }
        let config = WslServerConfig {
            id: id.clone(),
            distro: distro.to_string(),
        };
        let mut persisted = self.0.store.read();
        persisted.push(config.clone());
        self.0.store.write(&persisted);
        self.update(|state| {
            state.servers.push(WslServerItem {
                config: config.clone(),
                runtime: WslServerRuntime::Starting,
            })
        });
        let this = self.clone();
        self.spawn_task(async move { this.start_server(&id).await });
        Ok(config)
    }

    pub async fn remove_server(&self, id: &str) {
        let distro = self
            .state()
            .servers
            .iter()
            .find(|item| item.config.id == id)
            .map(|item| item.config.distro.clone());
        self.next_attempt(id);
        self.stop_internal(id).await;
        let remaining: Vec<WslServerConfig> = self
            .0
            .store
            .read()
            .into_iter()
            .filter(|item| item.id != id)
            .collect();
        self.0.store.write(&remaining);
        self.update(|state| {
            state.servers.retain(|item| item.config.id != id);
            if let Some(distro) = &distro {
                let (probes, checks) =
                    clear_wsl_distro_state(&state.distro_probes, &state.opencode_checks, distro);
                state.distro_probes = probes;
                state.opencode_checks = checks;
            }
        });
    }

    /// `stopAll` on quit: invalidate every start generation and stop (joined) every owned sidecar.
    pub async fn stop_all(&self) {
        let running: Vec<RunningSidecar> = {
            let mut inner = self.0.inner.lock();
            let ids: Vec<String> = inner
                .state
                .servers
                .iter()
                .map(|item| item.config.id.clone())
                .collect();
            ids.into_iter()
                .for_each(|id| *inner.attempts.entry(id).or_insert(0) += 1);
            inner.sidecars.drain().map(|(_, sidecar)| sidecar).collect()
        };
        futures::future::join_all(running.iter().map(|sidecar| sidecar.handle.stop())).await;
    }

    /// Quit-time teardown: stop sidecars, cancel in-flight spawns/jobs, join background tasks
    /// (bounded by `grace`, then aborted and joined).
    pub async fn shutdown(&self, grace: Duration) {
        self.stop_all().await;
        self.0.shutdown.cancel();
        let mut tasks = std::mem::take(&mut *self.0.tasks.lock());
        if tokio::time::timeout(grace, async { while tasks.join_next().await.is_some() {} })
            .await
            .is_err()
        {
            tasks.shutdown().await;
        }
        // Starts that completed during teardown may have registered sidecars; stop them too.
        self.stop_all().await;
    }
}
