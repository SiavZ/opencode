//! Purpose: local `SessionExecution` (packages/core/src/session/execution/local.ts): process-global
//! execution keyed by Session ID that resolves the stored Session's Location at drain start and runs
//! the Location-scoped `SessionRunner`. Future remote placement belongs here.
//! Scope: native, tokio. Owns one `RunCoordinator<SessionID, SessionError>`.
//! Uses:
//! rust/crates/
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/coordinator.rs
//! |-- session-runtime/src/runner/mod.rs
//! `-- session-runtime/src/store.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::future::FutureExt
//! |-- tokio::select
//! |-- tokio_util::sync::CancellationToken
//! `-- tracing::error
//! Used by:
//! rust/crates/
//! |-- runtime/src/location.rs
//! |-- runtime/src/process.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/tests/coordinator_execution.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: placement (store lookup + runner selection) happens per drain, never at
//! construction. A missing Session fails the drain with `NotFound` instead of panicking.
//! Interruption drops the runner future and is not logged; every other failure is logged with
//! the Session ID. No layer takes a Session ID.
//! Contract tests: session-runtime/tests/coordinator_execution.rs

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::FutureExt;
use opencode_session_api::operations::SessionExecution;
use opencode_session_api::{LocationRef, SessionError, SessionID, SessionInfo, SessionResult};
use tokio_util::sync::CancellationToken;

use crate::coordinator::{RunCoordinator, RunError};
use crate::runner::SessionRunner;
use crate::store::SessionStore;

/// `SessionStore.get` port used at drain start.
#[async_trait]
pub trait SessionLookup: Send + Sync {
    async fn get(&self, session_id: &SessionID) -> SessionResult<Option<SessionInfo>>;
}

#[async_trait]
impl SessionLookup for SessionStore {
    async fn get(&self, session_id: &SessionID) -> SessionResult<Option<SessionInfo>> {
        SessionStore::get(self, session_id).await
    }
}

/// `LocationServiceMap.get`: the Location-scoped runner for a stored Session's placement.
pub trait LocationRunners: Send + Sync {
    fn get(&self, location: &LocationRef) -> Arc<dyn SessionRunner>;
}

/// `SessionExecutionLocal`: current-process routing for implicit-local Locations.
#[derive(Clone)]
pub struct LocalSessionExecution {
    coordinator: RunCoordinator<SessionID, SessionError>,
}

impl LocalSessionExecution {
    /// Creates the coordinator on the current tokio runtime. Performs no lookups.
    pub fn new(store: Arc<dyn SessionLookup>, locations: Arc<dyn LocationRunners>) -> Self {
        let coordinator = RunCoordinator::new(move |session_id: SessionID, force, cancel| {
            drain(store.clone(), locations.clone(), session_id, force, cancel).boxed()
        });
        Self { coordinator }
    }

    /// Cancels every owned execution and refuses new work (scope close).
    pub async fn shutdown(&self) {
        self.coordinator.shutdown().await;
    }
}

#[async_trait]
impl SessionExecution for LocalSessionExecution {
    async fn active(&self) -> BTreeSet<SessionID> {
        self.coordinator.active().into_iter().collect()
    }

    async fn resume(&self, session_id: &SessionID) -> SessionResult<()> {
        self.coordinator
            .run(session_id.clone())
            .await
            .map_err(into_session_error)
    }

    async fn wake(&self, session_id: &SessionID) {
        self.coordinator.wake(session_id.clone());
    }

    async fn interrupt(&self, session_id: &SessionID) {
        self.coordinator.interrupt(session_id.clone()).await;
    }
}

async fn drain(
    store: Arc<dyn SessionLookup>,
    locations: Arc<dyn LocationRunners>,
    session_id: SessionID,
    force: bool,
    cancel: CancellationToken,
) -> SessionResult<()> {
    // The runner observes `cancel` itself so it can settle durable state (fail unsettled tools,
    // fail the active assistant) before returning `Cancelled`.
    let result = place_and_run(
        store.as_ref(),
        locations.as_ref(),
        &session_id,
        force,
        cancel,
    )
    .await;
    if let Err(error) = &result {
        tracing::error!(session_id = %session_id, error = %error, "Failed to drain Session");
    }
    result
}

async fn place_and_run(
    store: &dyn SessionLookup,
    locations: &dyn LocationRunners,
    session_id: &SessionID,
    force: bool,
    cancel: CancellationToken,
) -> SessionResult<()> {
    let session = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(SessionError::Cancelled),
        session = store.get(session_id) => session,
    }?
    .ok_or_else(|| SessionError::NotFound {
        session_id: session_id.clone(),
    })?;
    locations
        .get(&session.location)
        .run_cancellable(session_id, force, cancel)
        .await
}

fn into_session_error(error: RunError<SessionError>) -> SessionError {
    match error {
        RunError::Interrupted => SessionError::Cancelled,
        RunError::Panicked(message) => SessionError::Run {
            message: format!("execution panicked: {message}"),
            retryable: false,
        },
        RunError::Failed(error) => {
            Arc::try_unwrap(error).unwrap_or_else(|shared| clone_error(&shared))
        }
    }
}

/// Joined waiters share one failure. `SessionError` is not `Clone`, so rebuild it per waiter.
fn clone_error(error: &SessionError) -> SessionError {
    match error {
        SessionError::NotFound { session_id } => SessionError::NotFound {
            session_id: session_id.clone(),
        },
        SessionError::MessageNotFound {
            session_id,
            message_id,
        } => SessionError::MessageNotFound {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
        },
        SessionError::PromptConflict {
            session_id,
            message_id,
        } => SessionError::PromptConflict {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
        },
        SessionError::OperationUnavailable { operation } => SessionError::OperationUnavailable {
            operation: *operation,
        },
        SessionError::MessageDecode {
            session_id,
            message_id,
        } => SessionError::MessageDecode {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
        },
        SessionError::ContextSnapshotDecode {
            session_id,
            details,
        } => SessionError::ContextSnapshotDecode {
            session_id: session_id.clone(),
            details: details.clone(),
        },
        SessionError::ContextInitializationBlocked(message) => {
            SessionError::ContextInitializationBlocked(message.clone())
        }
        SessionError::Validation(message) => SessionError::Validation(message.clone()),
        SessionError::Cancelled => SessionError::Cancelled,
        SessionError::Run { message, retryable } => SessionError::Run {
            message: message.clone(),
            retryable: *retryable,
        },
        SessionError::Storage { message, retryable } => SessionError::Storage {
            message: message.clone(),
            retryable: *retryable,
        },
    }
}
