//! Purpose: concurrent local tool settlement for one provider turn (llm.ts `FiberSet` of
//! `toolMaterialization.settle` fibers plus `awaitToolFibers`). Each recorded call starts eagerly,
//! publishes its `tool-result` through the shared publisher, and is joined before continuation.
//! Scope: tasks owned by one attempt; never outlive it.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/runner/ports.rs
//! `-- session-runtime/src/runner/publish.rs
//! External:
//! |-- tokio::select
//! |-- tokio::sync::Mutex
//! |-- tokio::task::JoinSet
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/runner/turn.rs
//! `-- session-runtime/src/runner/turn/attempt.rs
//! Invariants: cancellation only interrupts the settle future, never a publication in progress
//! (baseline `uninterruptibleMask` around settle + publish); the first failure cancels the remaining
//! settlements; `join` returns only after every task finished.
//! Contract tests: session-runtime/tests/runner_turn_tools.rs

use std::sync::Arc;

use opencode_llm_api::LlmEvent;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::runner::ports::{MaterializedTools, SettleError, SettleInput};
use crate::runner::publish::LlmEventPublisher;

/// Why settlement did not complete.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum TurnFailure {
    /// A permission or question was declined: halt the loop.
    Declined,
    /// The turn was interrupted while tools were running.
    Interrupted,
    /// A settlement failed outside the model-facing channel (output store, publication).
    Failed(String),
}

pub(super) struct ToolTasks {
    publisher: Arc<Mutex<LlmEventPublisher>>,
    cancel: CancellationToken,
    set: JoinSet<Result<(), TurnFailure>>,
}

impl ToolTasks {
    pub(super) fn new(publisher: Arc<Mutex<LlmEventPublisher>>, cancel: CancellationToken) -> Self {
        Self {
            publisher,
            cancel,
            set: JoinSet::new(),
        }
    }

    /// Starts settling one recorded local call.
    pub(super) fn spawn(&mut self, tools: Arc<dyn MaterializedTools>, input: SettleInput) {
        let publisher = self.publisher.clone();
        let cancel = self.cancel.clone();
        self.set.spawn(async move {
            let (id, name) = (input.call_id.clone(), input.name.clone());
            let settled = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(TurnFailure::Interrupted),
                settled = tools.settle(input, cancel.clone()) => settled,
            };
            let settlement = match settled {
                Ok(settlement) => settlement,
                Err(SettleError::Declined(_)) => return Err(TurnFailure::Declined),
                Err(SettleError::Failed(message)) => return Err(TurnFailure::Failed(message)),
            };
            let event = LlmEvent::ToolResult {
                id,
                name,
                result: settlement.result,
                output: settlement.output,
                provider_executed: None,
                provider_metadata: None,
            };
            publisher
                .lock()
                .await
                .publish(&event, &settlement.output_paths)
                .await
                .map_err(|error| TurnFailure::Failed(error.to_string()))
        });
    }

    /// Interrupts running settlements and waits for every task (`FiberSet.clear`).
    pub(super) async fn cancel_all(&mut self) {
        self.cancel.cancel();
        while self.set.join_next().await.is_some() {}
    }

    /// Awaits every settlement. The first failure cancels the rest and is returned.
    pub(super) async fn join(&mut self) -> Result<(), TurnFailure> {
        let mut first: Option<TurnFailure> = None;
        while let Some(joined) = self.set.join_next().await {
            let outcome = match joined {
                Ok(outcome) => outcome,
                Err(error) if error.is_cancelled() => Err(TurnFailure::Interrupted),
                Err(error) => Err(TurnFailure::Failed(error.to_string())),
            };
            if let Err(failure) = outcome
                && first.is_none()
            {
                self.cancel.cancel();
                first = Some(failure);
            }
        }
        first.map_or(Ok(()), Err)
    }
}
