//! Purpose: updater state machine, port of `packages/desktop/src/main/updater-controller.ts`.
//! Owns the authoritative `UpdaterState`, coalesces concurrent checks into one in-flight
//! check, persists the ready target, and recovers to `error`/`ready` after failures.
//! Scope: async orchestration over an injected backend, persistence and stop hook.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater/state.rs
//! External:
//! |-- futures::future::{BoxFuture, FutureExt, Shared}
//! |-- parking_lot::Mutex
//! |-- tokio::task::JoinHandle
//! `-- tracing
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/updater.rs
//! |-- desktop-host/src/updater/feed.rs
//! `-- desktop-host/tests/updater_controller.rs
//! Invariants: at most one check in flight (later callers join it), `ready` short-circuits
//! checks, a failed check ends in `error` and the next check runs again, a failed install
//! returns to `ready` and surfaces the error, listeners run outside the state lock.
//! Contract tests: desktop-host/tests/updater_controller.rs

use std::sync::Arc;

use futures::future::{BoxFuture, FutureExt, Shared};
use parking_lot::Mutex;

use super::state::{UpdaterError, UpdaterReadyRecord, UpdaterState};

/// Result of `checkForUpdates()` (`updater-controller.ts:8`), flattened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckResult {
    pub is_update_available: bool,
    pub version: Option<String>,
}

/// Port of `UpdaterBackend` (`updater-controller.ts:7-11`).
pub trait UpdaterBackend: Send + Sync + 'static {
    fn check_for_updates(&self) -> BoxFuture<'_, Result<Option<CheckResult>, UpdaterError>>;
    fn download_update(&self) -> BoxFuture<'_, Result<(), UpdaterError>>;
    /// Hands off to the installer. Returning `Ok` means the process did not exit (the TS
    /// `quitAndInstall` can return without exiting), the controller then restores `ready`.
    fn quit_and_install(&self) -> Result<(), UpdaterError>;
}

/// Port of `UpdaterPersistence` (`updater-controller.ts:13-17`).
pub trait UpdaterPersistence: Send + Sync + 'static {
    fn get(&self) -> Result<Option<UpdaterReadyRecord>, UpdaterError>;
    fn set(&self, value: &UpdaterReadyRecord) -> Result<(), UpdaterError>;
    fn clear(&self) -> Result<(), UpdaterError>;
}

/// Stops the local server and other children before install (`index.ts` passes `killSidecar`).
pub type StopFn = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct UpdaterInput {
    pub enabled: bool,
    pub current_version: String,
    pub backend: Arc<dyn UpdaterBackend>,
    pub persistence: Arc<dyn UpdaterPersistence>,
    pub stop: StopFn,
}

type Listener = Arc<dyn Fn(&UpdaterState) + Send + Sync>;
type PendingCheck = Shared<BoxFuture<'static, UpdaterState>>;

struct Inner {
    state: UpdaterState,
    pending: Option<PendingCheck>,
    listeners: Vec<(u64, Listener)>,
    next_listener: u64,
}

struct Shared_ {
    enabled: bool,
    current_version: String,
    backend: Arc<dyn UpdaterBackend>,
    persistence: Arc<dyn UpdaterPersistence>,
    stop: StopFn,
    inner: Mutex<Inner>,
}

/// Cheap to clone, all clones share one state machine.
#[derive(Clone)]
pub struct UpdaterController {
    shared: Arc<Shared_>,
}

impl std::fmt::Debug for UpdaterController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterController")
            .field("state", &self.state())
            .finish()
    }
}

impl UpdaterController {
    pub fn new(input: UpdaterInput) -> Self {
        let state = if input.enabled {
            UpdaterState::Idle
        } else {
            UpdaterState::Disabled
        };
        Self {
            shared: Arc::new(Shared_ {
                enabled: input.enabled,
                current_version: input.current_version,
                backend: input.backend,
                persistence: input.persistence,
                stop: input.stop,
                inner: Mutex::new(Inner {
                    state,
                    pending: None,
                    listeners: Vec::new(),
                    next_listener: 0,
                }),
            }),
        }
    }

    /// `UPDATER_ENABLED = app.isPackaged && CHANNEL !== "dev"` (`constants.ts:7`).
    pub fn enabled_for(packaged: bool, build_channel: &str) -> bool {
        packaged && build_channel != "dev"
    }

    pub fn state(&self) -> UpdaterState {
        self.shared.inner.lock().state.clone()
    }

    /// Registers `listener`, immediately calls it with the current state, returns a disposer.
    pub fn subscribe(
        &self,
        listener: impl Fn(&UpdaterState) + Send + Sync + 'static,
    ) -> impl FnOnce() + Send + 'static {
        let listener: Listener = Arc::new(listener);
        let (id, current) = {
            let mut inner = self.shared.inner.lock();
            let id = inner.next_listener;
            inner.next_listener += 1;
            inner.listeners.push((id, listener.clone()));
            (id, inner.state.clone())
        };
        listener(&current);
        let weak = Arc::downgrade(&self.shared);
        move || {
            if let Some(shared) = weak.upgrade() {
                shared.inner.lock().listeners.retain(|(lid, _)| *lid != id);
            }
        }
    }

    /// Clears a persisted target that is already installed, then checks (`start()`).
    pub async fn start(&self) -> UpdaterState {
        let ready = self.shared.persistence.get();
        match ready {
            Ok(Some(record)) if record.version == self.shared.current_version => {
                if let Err(error) = self.shared.persistence.clear() {
                    tracing::warn!(%error, "failed to clear installed updater target");
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "failed to read persisted updater target"),
        }
        self.check().await
    }

    /// Coalesced check. Disabled and ready states return immediately.
    pub async fn check(&self) -> UpdaterState {
        let pending = {
            let mut inner = self.shared.inner.lock();
            if !self.shared.enabled {
                return inner.state.clone();
            }
            if matches!(inner.state, UpdaterState::Ready { .. }) {
                return inner.state.clone();
            }
            if let Some(pending) = &inner.pending {
                pending.clone()
            } else {
                let pending = self.spawn_check();
                inner.pending = Some(pending.clone());
                pending
            }
        };
        pending.await
    }

    /// The check runs on an owned task so that a dropped caller cannot strand the state in
    /// `checking`. Every waiter joins the same task through the shared handle.
    fn spawn_check(&self) -> PendingCheck {
        let shared = self.shared.clone();
        let handle = tokio::spawn(async move {
            let result = run_check(&shared).await;
            let next = match result {
                Ok(state) => state,
                Err(error) => transition(
                    &shared,
                    UpdaterState::Error {
                        message: error.to_string(),
                    },
                ),
            };
            shared.inner.lock().pending = None;
            next
        });
        let weak = Arc::downgrade(&self.shared);
        async move {
            match handle.await {
                Ok(state) => state,
                Err(join) => {
                    let message = format!("updater check task failed: {join}");
                    match weak.upgrade() {
                        Some(shared) => {
                            shared.inner.lock().pending = None;
                            transition(&shared, UpdaterState::Error { message })
                        }
                        None => UpdaterState::Error { message },
                    }
                }
            }
        }
        .boxed()
        .shared()
    }

    /// Stops children, hands off to the installer, restores `ready` if the process survives.
    pub async fn install(&self) -> Result<(), UpdaterError> {
        let version = match self.state() {
            UpdaterState::Ready { version } => version,
            _ => return Err(UpdaterError::NotReady),
        };
        transition(
            &self.shared,
            UpdaterState::Installing {
                version: version.clone(),
            },
        );
        let result = match (self.shared.stop)().await {
            Ok(()) => self.shared.backend.quit_and_install(),
            Err(message) => Err(UpdaterError::Stop(message)),
        };
        transition(&self.shared, UpdaterState::Ready { version });
        result
    }
}

async fn run_check(shared: &Shared_) -> Result<UpdaterState, UpdaterError> {
    transition(shared, UpdaterState::Checking);
    let result = shared.backend.check_for_updates().await?;
    let version = result
        .as_ref()
        .filter(|result| result.is_update_available)
        .and_then(|result| result.version.clone())
        .filter(|version| !version.is_empty() && *version != shared.current_version);
    let Some(version) = version else {
        shared.persistence.clear()?;
        return Ok(transition(shared, UpdaterState::UpToDate));
    };
    transition(
        shared,
        UpdaterState::Downloading {
            version: version.clone(),
            percent: None,
        },
    );
    shared.backend.download_update().await?;
    shared.persistence.set(&UpdaterReadyRecord {
        version: version.clone(),
    })?;
    Ok(transition(shared, UpdaterState::Ready { version }))
}

fn transition(shared: &Shared_, next: UpdaterState) -> UpdaterState {
    let listeners = {
        let mut inner = shared.inner.lock();
        tracing::info!(
            from = inner.state.status(),
            to = next.status(),
            "updater state changed"
        );
        inner.state = next.clone();
        inner
            .listeners
            .iter()
            .map(|(_, l)| l.clone())
            .collect::<Vec<_>>()
    };
    listeners.iter().for_each(|listener| listener(&next));
    next
}
