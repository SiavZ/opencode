//! Purpose: one-shot shared startup readiness (`serverReady` in the Electron main process) whose
//! waiters resolve on success AND on failure, including panics and abandoned loading tasks.
//! Scope: in-process synchronization only. Does not spawn or own the loading task.
//! Uses:
//! `-- none
//! External:
//! |-- futures::FutureExt
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- tokio::sync::watch
//! |-- watch::Sender
//! `-- watch::Sender::new
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lifecycle.rs
//! `-- desktop-host/tests/lifecycle_readiness.rs
//! Invariants: first completion wins, later completions are ignored and reported as `false`;
//! late waiters observe the stored outcome; a loading task that fails, panics or is dropped
//! before completing fails the readiness so no waiter hangs.
//! Contract tests: desktop-host/tests/lifecycle_readiness.rs

use std::fmt::Display;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Payload resolved by `awaitInitialization` (`preload/types.ts:19-23`). Wire shape: `username`
/// and `password` are serialized as `null` when absent, never omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerReadyData {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Why initialization did not produce a value. Mirrors the Effect `Cause` kinds forwarded by
/// `forwardInitializationFailure` (`initialization.ts:3-6`): failure, defect, interruption.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InitializationError {
    #[error("initialization failed: {0}")]
    Failed(String),
    #[error("initialization panicked: {0}")]
    Panicked(String),
    #[error("initialization was cancelled before completing")]
    Cancelled,
}

/// Shared one-shot readiness cell. Cloning shares the same cell.
#[derive(Debug)]
pub struct Readiness<T> {
    tx: Arc<watch::Sender<Option<Result<T, InitializationError>>>>,
}

impl<T> Clone for Readiness<T> {
    fn clone(&self) -> Self {
        Self {
            tx: Arc::clone(&self.tx),
        }
    }
}

impl<T: Clone> Default for Readiness<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Readiness<T> {
    pub fn new() -> Self {
        Self {
            tx: Arc::new(watch::Sender::new(None)),
        }
    }

    /// Completes with a value. Returns `false` if already completed.
    pub fn succeed(&self, value: T) -> bool {
        self.complete(Ok(value))
    }

    /// Completes with a failure. Returns `false` if already completed.
    pub fn fail(&self, error: InitializationError) -> bool {
        self.complete(Err(error))
    }

    fn complete(&self, outcome: Result<T, InitializationError>) -> bool {
        self.tx.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some(outcome);
            true
        })
    }

    /// Current outcome without waiting.
    pub fn try_get(&self) -> Option<Result<T, InitializationError>> {
        self.tx.borrow().clone()
    }

    pub fn is_complete(&self) -> bool {
        self.tx.borrow().is_some()
    }

    /// Waits for the outcome. Resolves immediately for late waiters.
    pub async fn wait(&self) -> Result<T, InitializationError> {
        let mut rx = self.tx.subscribe();
        // The sender lives as long as `self`, so `wait_for` cannot observe a closed channel.
        match rx.wait_for(Option::is_some).await {
            Ok(guard) => guard.clone().unwrap_or(Err(InitializationError::Cancelled)),
            Err(_) => Err(InitializationError::Cancelled),
        }
    }

    /// Runs the loading task and forwards every non-success exit into `self`, the port of
    /// `forwardInitializationFailure`. Errors and panics fail the readiness; if the returned
    /// future is dropped before the task finishes, the readiness fails with `Cancelled`.
    /// Success does not complete the readiness: the task calls `succeed` itself, as in the
    /// baseline where the task resolves `serverReady` midway and keeps running health checks.
    pub async fn forward_failure<R, E, F>(&self, task: F) -> Result<R, InitializationError>
    where
        E: Display,
        F: Future<Output = Result<R, E>>,
    {
        let guard = CancelGuard {
            readiness: self,
            armed: true,
        };
        let outcome = AssertUnwindSafe(task).catch_unwind().await;
        let mut guard = guard;
        guard.armed = false;
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                let error = InitializationError::Failed(error.to_string());
                self.fail(error.clone());
                Err(error)
            }
            Err(panic) => {
                let error = InitializationError::Panicked(panic_message(panic.as_ref()));
                self.fail(error.clone());
                Err(error)
            }
        }
    }
}

struct CancelGuard<'a, T: Clone> {
    readiness: &'a Readiness<T>,
    armed: bool,
}

impl<T: Clone> Drop for CancelGuard<'_, T> {
    fn drop(&mut self) {
        if self.armed {
            self.readiness.fail(InitializationError::Cancelled);
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "non-string panic payload".to_string()
}
