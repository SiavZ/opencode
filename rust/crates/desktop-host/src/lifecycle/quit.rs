//! Purpose: quit and relaunch coordination (`main/index.ts:167-177,224-251`, `windows.ts:48-74`):
//! one quit attempt at a time, ordered before-quit/will-quit phases of joined async cleanup hooks
//! with per-hook timeouts and cancellation, and restore-on-failure.
//! Scope: coordination state machine. The shell performs the final native exit/relaunch call
//! after `request` returns `QuitDecision::Exit`.
//! Uses:
//! `-- none
//! External:
//! |-- futures::future::{join_all, BoxFuture}
//! |-- parking_lot::Mutex
//! |-- thiserror::Error
//! |-- tokio::sync::watch
//! |-- tokio::time::timeout
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lifecycle.rs
//! `-- desktop-host/tests/lifecycle_quit.rs
//! Invariants: `is_quitting` is true only while an attempt runs or after it decided to exit; a
//! cancelled or aborted attempt restores `Running` and clears the relaunch flag; concurrent
//! requests join the active attempt and a relaunch request is sticky for that attempt; a dropped
//! driver future publishes `Cancelled` and restores state so no joiner hangs; the lock is never
//! held across an await.
//! Contract tests: desktop-host/tests/lifecycle_quit.rs

use std::sync::Arc;
use std::time::Duration;

use futures::future::{BoxFuture, join_all};
use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Ordered quit phases mirroring Electron `before-quit` then `will-quit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuitPhase {
    BeforeQuit,
    WillQuit,
}

/// What a hook failure means for the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPolicy {
    /// Failure or timeout is logged and quitting continues. Baseline `stopSidecars` uses this:
    /// `stopSidecars().finally(() => app.quit())`.
    BestEffort,
    /// Failure or timeout aborts the attempt and restores the running state (for example a
    /// draft flush that must not be lost).
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitReason {
    User,
    Signal,
    SessionEnd,
    Relaunch,
    Updater,
    AllWindowsClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookError {
    #[error("hook {name} failed: {message}")]
    Failed { name: String, message: String },
    #[error("hook {name} timed out after {timeout:?}")]
    TimedOut { name: String, timeout: Duration },
    #[error("hook {name} was cancelled")]
    Cancelled { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitDecision {
    /// All required hooks succeeded. The shell exits, relaunching first when `relaunch`.
    Exit {
        relaunch: bool,
        best_effort_failures: Vec<HookError>,
    },
    /// A required hook failed. State is restored to running.
    Aborted { error: HookError },
    /// `cancel` was called or the driver was dropped. State is restored to running.
    Cancelled,
}

type HookFn = dyn Fn(CancellationToken) -> BoxFuture<'static, Result<(), String>> + Send + Sync;

struct Hook {
    name: String,
    phase: QuitPhase,
    policy: HookPolicy,
    timeout: Duration,
    run: Arc<HookFn>,
}

enum Entry {
    Done(QuitDecision),
    Join(watch::Receiver<Option<QuitDecision>>),
    Drive(CancellationToken, watch::Sender<Option<QuitDecision>>),
}

enum State {
    Running,
    Quitting {
        relaunch: bool,
        token: CancellationToken,
        outcome: watch::Receiver<Option<QuitDecision>>,
    },
    Exiting {
        decision: QuitDecision,
    },
}

#[derive(Clone, Default)]
pub struct QuitCoordinator {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<StateCell>,
    hooks: Mutex<Vec<Hook>>,
    quitting: QuittingFlag,
}

struct StateCell(State);

impl Default for StateCell {
    fn default() -> Self {
        Self(State::Running)
    }
}

struct QuittingFlag(watch::Sender<bool>);

impl Default for QuittingFlag {
    fn default() -> Self {
        Self(watch::Sender::new(false))
    }
}

impl QuitCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a cleanup hook. Hooks in one phase run concurrently and are joined before the
    /// next phase starts. The hook must observe the token and stop promptly when cancelled.
    pub fn register<F, Fut>(
        &self,
        name: impl Into<String>,
        phase: QuitPhase,
        policy: HookPolicy,
        timeout: Duration,
        hook: F,
    ) where
        F: Fn(CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.inner.hooks.lock().push(Hook {
            name: name.into(),
            phase,
            policy,
            timeout,
            run: Arc::new(move |token| Box::pin(hook(token))),
        });
    }

    /// Mirrors `registry.setQuitting` consumers (window close handlers treat close as quit).
    pub fn is_quitting(&self) -> bool {
        *self.inner.quitting.0.borrow()
    }

    pub fn subscribe_quitting(&self) -> watch::Receiver<bool> {
        self.inner.quitting.0.subscribe()
    }

    /// Cancels the active attempt, if any. Returns whether an attempt was cancelled.
    pub fn cancel(&self) -> bool {
        match &self.inner.state.lock().0 {
            State::Quitting { token, .. } => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    /// Relaunch request: same as `request(QuitReason::Relaunch)`.
    pub async fn relaunch(&self) -> QuitDecision {
        self.request(QuitReason::Relaunch).await
    }

    /// Starts a quit attempt or joins the active one.
    pub async fn request(&self, reason: QuitReason) -> QuitDecision {
        let relaunch = reason == QuitReason::Relaunch;
        let entry = {
            let mut cell = self.inner.state.lock();
            match &mut cell.0 {
                State::Exiting { decision } => Entry::Done(decision.clone()),
                State::Quitting {
                    relaunch: sticky,
                    outcome,
                    ..
                } => {
                    *sticky |= relaunch;
                    Entry::Join(outcome.clone())
                }
                State::Running => {
                    let (tx, rx) = watch::channel(None);
                    let token = CancellationToken::new();
                    cell.0 = State::Quitting {
                        relaunch,
                        token: token.clone(),
                        outcome: rx,
                    };
                    self.inner.quitting.0.send_replace(true);
                    Entry::Drive(token, tx)
                }
            }
        };
        let mut outcome = match entry {
            Entry::Done(decision) => return decision,
            Entry::Drive(token, tx) => {
                tracing::info!(?reason, "quit attempt started");
                return self.drive(token, tx).await;
            }
            Entry::Join(outcome) => outcome,
        };
        match outcome.wait_for(Option::is_some).await {
            Ok(decision) => decision.clone().unwrap_or(QuitDecision::Cancelled),
            Err(_) => QuitDecision::Cancelled,
        }
    }

    async fn drive(
        &self,
        token: CancellationToken,
        tx: watch::Sender<Option<QuitDecision>>,
    ) -> QuitDecision {
        let mut guard = DriverGuard {
            inner: &self.inner,
            tx: &tx,
            done: false,
        };
        let mut best_effort_failures = Vec::new();
        let mut aborted = None;
        for phase in [QuitPhase::BeforeQuit, QuitPhase::WillQuit] {
            if token.is_cancelled() {
                break;
            }
            let hooks: Vec<_> = self
                .inner
                .hooks
                .lock()
                .iter()
                .filter(|hook| hook.phase == phase)
                .map(|hook| {
                    (
                        hook.name.clone(),
                        hook.policy,
                        hook.timeout,
                        Arc::clone(&hook.run),
                    )
                })
                .collect();
            let results = join_all(hooks.into_iter().map(|(name, policy, limit, run)| {
                let child = token.child_token();
                async move { (policy, run_hook(name, limit, run, child).await) }
            }))
            .await;
            for (policy, result) in results {
                let Err(error) = result else { continue };
                tracing::warn!(%error, ?policy, "quit hook did not complete");
                match policy {
                    HookPolicy::BestEffort => best_effort_failures.push(error),
                    HookPolicy::Required if aborted.is_none() => aborted = Some(error),
                    HookPolicy::Required => best_effort_failures.push(error),
                }
            }
            if aborted.is_some() {
                break;
            }
        }
        let decision = if token.is_cancelled() {
            QuitDecision::Cancelled
        } else if let Some(error) = aborted {
            QuitDecision::Aborted { error }
        } else {
            let relaunch = matches!(
                &self.inner.state.lock().0,
                State::Quitting { relaunch: true, .. }
            );
            QuitDecision::Exit {
                relaunch,
                best_effort_failures,
            }
        };
        guard.finish(decision.clone());
        decision
    }
}

async fn run_hook(
    name: String,
    limit: Duration,
    run: Arc<HookFn>,
    token: CancellationToken,
) -> Result<(), HookError> {
    let work = run(token.clone());
    tokio::select! {
        biased;
        () = token.cancelled() => Err(HookError::Cancelled { name }),
        result = tokio::time::timeout(limit, work) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(HookError::Failed { name, message }),
            Err(_) => {
                // Dropping the future cancels the hook's own work; it does not prove that any
                // process the hook was stopping has exited.
                token.cancel();
                Err(HookError::TimedOut { name, timeout: limit })
            }
        },
    }
}

/// Publishes the outcome and settles state, also when the driver future is dropped mid-attempt.
struct DriverGuard<'a> {
    inner: &'a Inner,
    tx: &'a watch::Sender<Option<QuitDecision>>,
    done: bool,
}

impl DriverGuard<'_> {
    fn finish(&mut self, decision: QuitDecision) {
        self.done = true;
        let mut cell = self.inner.state.lock();
        if let QuitDecision::Exit { .. } = decision {
            cell.0 = State::Exiting {
                decision: decision.clone(),
            };
        } else {
            cell.0 = State::Running;
            self.inner.quitting.0.send_replace(false);
        }
        drop(cell);
        self.tx.send_replace(Some(decision));
    }
}

impl Drop for DriverGuard<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let State::Quitting { token, .. } = &self.inner.state.lock().0 {
            token.cancel();
        }
        self.finish(QuitDecision::Cancelled);
    }
}
