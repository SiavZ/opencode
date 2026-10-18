//! Purpose: per-Session legacy run ownership: the `Runner` state machine of
//! packages/opencode/src/effect/runner.ts (Idle | Running | Shell | ShellThenRun, join, busy shell
//! rejection, cancel with interrupt fallback) and `SessionRunState` + `SessionStatus` of
//! packages/opencode/src/session/{run-state,status}.ts.
//! Scope: process-local, one `RunState` per Location instance. Runs are owned tokio tasks.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- tokio::sync::watch
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! `-- session-runtime/tests/legacy_shell.rs
//! Invariants: at most one run or shell per Session. `ensure_running` joins an active run, queues
//! behind a shell (`ShellThenRun`) and otherwise starts one. A shell while busy fails with `Busy`.
//! `cancel` waits for the shell to become ready, signals it, joins it, fails pending runs with
//! `Cancelled` and resolves interrupted awaiters through the runner's `on_interrupt` fallback
//! (`lastAssistant`). The idle hook (status idle + runner removal) runs exactly once per busy period.
//! No std lock is held across an await.
//! Contract tests: session-runtime/tests/legacy_prompt.rs, session-runtime/tests/legacy_shell.rs

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::store::{LegacyStore, SESSION_IDLE, SESSION_STATUS};
use super::types::{LegacyError, WithParts};

type Outcome = Result<WithParts, LegacyError>;
type Done = watch::Sender<Option<Outcome>>;
/// Lazily computed result for interrupted awaiters (`onInterrupt`).
pub type Fallback = Arc<dyn Fn() -> BoxFuture<'static, Outcome> + Send + Sync>;
/// Shell work: receives the cancel signal and the ready latch it must open once its transcript
/// rows are written.
pub type ShellWork =
    Box<dyn FnOnce(CancellationToken, Ready) -> BoxFuture<'static, Outcome> + Send>;

/// `Latch` opened by shell work after the user/assistant/tool rows are durable.
#[derive(Clone)]
pub struct Ready(Arc<watch::Sender<bool>>);

impl Ready {
    fn new() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
    pub fn open(&self) {
        self.0.send_replace(true);
    }
    async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|open| *open).await;
    }
}

struct ShellHandle {
    id: u64,
    cancel: CancellationToken,
    ready: Ready,
    done: watch::Receiver<Option<Outcome>>,
}

enum State {
    Idle,
    Running {
        id: u64,
        done: Arc<Done>,
        abort: tokio::task::AbortHandle,
    },
    Shell(ShellHandle),
    ShellThenRun {
        shell: ShellHandle,
        run_id: u64,
        done: Arc<Done>,
        work: BoxFuture<'static, Outcome>,
    },
}

/// Cancels a Session's background jobs and its descendants (`cancelBackgroundJobs`).
#[async_trait]
pub trait JobCanceller: Send + Sync {
    async fn cancel_tree(&self, session_id: &str);
}

#[async_trait]
impl JobCanceller for crate::children::Children {
    async fn cancel_tree(&self, session_id: &str) {
        crate::children::Children::cancel_tree(self, session_id).await;
    }
}

/// `SessionStatus.Info` (`idle | busy | retry`).
pub type Status = Value;

/// `SessionStatus`: in-memory status map that publishes `session.status` (and `session.idle`).
#[derive(Clone)]
pub struct SessionStatus {
    store: LegacyStore,
    map: Arc<Mutex<HashMap<String, Status>>>,
}

impl SessionStatus {
    pub fn new(store: LegacyStore) -> Self {
        Self {
            store,
            map: Arc::default(),
        }
    }

    pub fn get(&self, session: &str) -> Status {
        self.lock()
            .get(session)
            .cloned()
            .unwrap_or_else(|| json!({ "type": "idle" }))
    }

    pub fn list(&self) -> HashMap<String, Status> {
        self.lock().clone()
    }

    pub async fn set(&self, session: &str, status: Status) -> Result<(), LegacyError> {
        self.store
            .publish(
                &SESSION_STATUS,
                json!({ "sessionID": session, "status": status }),
            )
            .await?;
        if status.get("type").and_then(Value::as_str) == Some("idle") {
            self.store
                .publish(&SESSION_IDLE, json!({ "sessionID": session }))
                .await?;
            self.lock().remove(session);
            return Ok(());
        }
        self.lock().insert(session.to_owned(), status);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Status>> {
        self.map.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

/// One Session's `Runner`.
struct Runner {
    session: String,
    owner: RunState,
    state: Mutex<State>,
    on_interrupt: Fallback,
    ids: std::sync::atomic::AtomicU64,
}

impl Runner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn next(&self) -> u64 {
        self.ids.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    fn busy(&self) -> bool {
        !matches!(*self.lock(), State::Idle)
    }

    async fn idle(&self) {
        self.owner.on_idle(&self.session).await;
    }

    async fn await_done(&self, mut receiver: watch::Receiver<Option<Outcome>>) -> Outcome {
        let outcome = match receiver.wait_for(Option::is_some).await {
            Ok(value) => value.clone().unwrap_or(Err(LegacyError::Cancelled)),
            Err(_) => Err(LegacyError::Cancelled),
        };
        match outcome {
            Err(LegacyError::Cancelled) => (self.on_interrupt)().await,
            other => other,
        }
    }

    /// Starts `work` as the current run. Caller holds no lock.
    fn start_run(
        self: &Arc<Self>,
        id: u64,
        done: Arc<Done>,
        work: BoxFuture<'static, Outcome>,
    ) -> State {
        let runner = self.clone();
        let sender = done.clone();
        let task = tokio::spawn(async move {
            let outcome = work.await;
            runner.finish_run(id, &sender, outcome).await;
        });
        State::Running {
            id,
            done,
            abort: task.abort_handle(),
        }
    }

    async fn finish_run(&self, id: u64, done: &Done, outcome: Outcome) {
        let current = {
            let mut state = self.lock();
            let current = matches!(*state, State::Running { id: run, .. } if run == id);
            if current {
                *state = State::Idle;
            }
            current
        };
        if current {
            self.idle().await;
        }
        done.send_replace(Some(outcome));
    }

    async fn ensure_running(self: &Arc<Self>, work: BoxFuture<'static, Outcome>) -> Outcome {
        let receiver = {
            let mut state = self.lock();
            match &mut *state {
                State::Running { done, .. } | State::ShellThenRun { done, .. } => done.subscribe(),
                State::Shell(_) => {
                    let State::Shell(shell) = std::mem::replace(&mut *state, State::Idle) else {
                        unreachable!()
                    };
                    let done = Arc::new(watch::channel(None).0);
                    let receiver = done.subscribe();
                    *state = State::ShellThenRun {
                        shell,
                        run_id: self.next(),
                        done,
                        work,
                    };
                    receiver
                }
                State::Idle => {
                    let done = Arc::new(watch::channel(None).0);
                    let receiver = done.subscribe();
                    let id = self.next();
                    *state = self.start_run(id, done, work);
                    receiver
                }
            }
        };
        self.await_done(receiver).await
    }

    async fn start_shell(self: &Arc<Self>, work: ShellWork) -> Outcome {
        let (id, cancel, ready, sender, receiver) = {
            let mut state = self.lock();
            if !matches!(*state, State::Idle) {
                return Err(LegacyError::Busy(self.session.clone()));
            }
            let (sender, receiver) = watch::channel(None);
            let shell = ShellHandle {
                id: self.next(),
                cancel: CancellationToken::new(),
                ready: Ready::new(),
                done: receiver.clone(),
            };
            let parts = (
                shell.id,
                shell.cancel.clone(),
                shell.ready.clone(),
                sender,
                receiver,
            );
            *state = State::Shell(shell);
            parts
        };
        self.owner
            .status
            .set(&self.session, json!({ "type": "busy" }))
            .await?;
        let runner = self.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            let latch = ready.clone();
            let outcome = work(token, ready).await;
            latch.open();
            sender.send_replace(Some(outcome));
            runner.finish_shell(id).await;
        });
        let mut receiver = receiver;
        let outcome = receiver
            .wait_for(Option::is_some)
            .await
            .map(|value| value.clone().unwrap_or(Err(LegacyError::Cancelled)))
            .unwrap_or(Err(LegacyError::Cancelled));
        if cancel.is_cancelled() && !matches!(outcome, Err(LegacyError::Process(_))) {
            return (self.on_interrupt)().await;
        }
        outcome
    }

    async fn finish_shell(self: &Arc<Self>, id: u64) {
        let idle = {
            let mut state = self.lock();
            match std::mem::replace(&mut *state, State::Idle) {
                State::Shell(shell) if shell.id == id => true,
                State::ShellThenRun {
                    shell,
                    run_id,
                    done,
                    work,
                } if shell.id == id => {
                    *state = self.start_run(run_id, done, work);
                    false
                }
                other => {
                    *state = other;
                    false
                }
            }
        };
        if idle {
            self.idle().await;
        }
    }

    async fn stop_shell(shell: ShellHandle) {
        shell.ready.wait().await;
        shell.cancel.cancel();
        let mut done = shell.done;
        let _ = done.wait_for(Option::is_some).await;
    }

    async fn cancel(&self) {
        let previous = std::mem::replace(&mut *self.lock(), State::Idle);
        match previous {
            State::Idle => return,
            State::Running { done, abort, .. } => {
                abort.abort();
                done.send_replace(Some(Err(LegacyError::Cancelled)));
            }
            State::Shell(shell) => Self::stop_shell(shell).await,
            State::ShellThenRun { shell, done, .. } => {
                Self::stop_shell(shell).await;
                done.send_replace(Some(Err(LegacyError::Cancelled)));
            }
        }
        if matches!(*self.lock(), State::Idle) {
            self.idle().await;
        }
    }
}

/// `SessionRunState`: one `Runner` per busy Session plus status and job cancellation.
#[derive(Clone)]
pub struct RunState {
    runners: Arc<Mutex<HashMap<String, Arc<Runner>>>>,
    status: SessionStatus,
    jobs: Option<Arc<dyn JobCanceller>>,
}

impl RunState {
    pub fn new(status: SessionStatus, jobs: Option<Arc<dyn JobCanceller>>) -> Self {
        Self {
            runners: Arc::default(),
            status,
            jobs,
        }
    }

    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Runner>>> {
        self.runners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    async fn on_idle(&self, session: &str) {
        self.lock().remove(session);
        // Status publish failures are logged by the bus in the baseline; nothing to propagate.
        let _ = self.status.set(session, json!({ "type": "idle" })).await;
    }

    fn runner(&self, session: &str, on_interrupt: Fallback) -> Arc<Runner> {
        self.lock()
            .entry(session.to_owned())
            .or_insert_with(|| {
                Arc::new(Runner {
                    session: session.to_owned(),
                    owner: self.clone(),
                    state: Mutex::new(State::Idle),
                    on_interrupt,
                    ids: Default::default(),
                })
            })
            .clone()
    }

    /// Whether the Session currently runs a loop or shell.
    pub fn busy(&self, session: &str) -> bool {
        self.lock().get(session).is_some_and(|runner| runner.busy())
    }

    /// `assertNotBusy`.
    pub fn assert_not_busy(&self, session: &str) -> Result<(), LegacyError> {
        if self.busy(session) {
            return Err(LegacyError::Busy(session.to_owned()));
        }
        Ok(())
    }

    /// `cancel`: background job tree first, then the runner (or status idle when none).
    pub async fn cancel(&self, session: &str) {
        if let Some(jobs) = &self.jobs {
            jobs.cancel_tree(session).await;
        }
        let runner = self.lock().get(session).cloned();
        match runner {
            Some(runner) => runner.cancel().await,
            None => {
                let _ = self.status.set(session, json!({ "type": "idle" })).await;
            }
        }
    }

    /// `ensureRunning`.
    pub async fn ensure_running(
        &self,
        session: &str,
        on_interrupt: Fallback,
        work: BoxFuture<'static, Outcome>,
    ) -> Outcome {
        self.runner(session, on_interrupt)
            .ensure_running(work)
            .await
    }

    /// `startShell`: fails with `Busy` unless idle.
    pub async fn start_shell(
        &self,
        session: &str,
        on_interrupt: Fallback,
        work: ShellWork,
    ) -> Outcome {
        self.runner(session, on_interrupt).start_shell(work).await
    }

    /// Cancels every runner (`InstanceState` finalizer).
    pub async fn shutdown(&self) {
        let runners: Vec<_> = self.lock().values().cloned().collect();
        for runner in runners {
            runner.cancel().await;
        }
        self.lock().clear();
    }
}
