//! Purpose: llm.ts `SessionRunner` layer: `LlmSessionRunner` drains one Session. Promotes
//! steers then queued inputs at their boundaries, runs provider turns until no continuation is
//! needed, resets the agent step allowance on promotion, and routes compaction transitions
//! (`runTurn` / `runAfterOverflowCompaction`).
//! Scope: Location-scoped runner. One instance per Location; the process-global coordinator
//! serializes drains per Session.
//! Uses:
//! rust/crates/
//! |-- context/src/source/error.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/runner/turn/request.rs
//! |-- session-runtime/src/runner/turn/tools.rs
//! `-- session-runtime/src/store.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/runner/mod.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: advisory runs (`force=false`) without pending input return before any write;
//! interrupted tools from a previous owner fail before the first turn; promotion is steer after
//! the first turn and one queue input only at would-be idle; a second overflow after overflow
//! compaction is an error.
//! Contract tests: session-runtime/tests/runner_turn.rs, session-runtime/tests/runner_turn_steps.rs,
//! session-runtime/tests/runner_turn_tools.rs

mod attempt;
mod request;
mod tools;

use std::sync::Arc;

use async_trait::async_trait;
use opencode_context::source::ContextError;
use opencode_schema::session_event::{ToolFailed, ToolProvider};
use opencode_schema::session_message::{AssistantContent, ToolState, UnknownError};
use opencode_schema::values::DateTimeUtc;
use opencode_session_api::{Delivery, LocationRef, Message, SessionError, SessionID, SessionInfo};
use opencode_storage_api::{DatabaseExt, Sql};
use tokio_util::sync::CancellationToken;

use attempt::Attempt;
pub use request::prompt_cache_key;

use crate::agents::Agents;
use crate::inbox;
use crate::projection::{SessionEvents, publish_session_event};
use crate::runner::SessionRunner;
use crate::runner::model::SessionRunnerModel;
use crate::runner::ports::{
    Compaction, LlmClient, Snapshots, SystemContextLoader, ToolMaterializer,
};
use crate::store::SessionStore;

/// Collaborators of one Location's runner (the llm.ts layer dependencies).
pub struct LlmSessionRunner {
    pub(crate) events: Arc<dyn SessionEvents>,
    pub(crate) llm: Arc<dyn LlmClient>,
    pub(crate) agents: Agents,
    pub(crate) tools: Arc<dyn ToolMaterializer>,
    pub(crate) models: Arc<dyn SessionRunnerModel>,
    pub(crate) context: Arc<dyn SystemContextLoader>,
    pub(crate) compaction: Arc<dyn Compaction>,
    pub(crate) snapshots: Arc<dyn Snapshots>,
    pub(crate) location: LocationRef,
    store: SessionStore,
}

/// Constructor input for [`LlmSessionRunner`].
pub struct RunnerDeps {
    pub events: Arc<dyn SessionEvents>,
    pub llm: Arc<dyn LlmClient>,
    pub agents: Agents,
    pub tools: Arc<dyn ToolMaterializer>,
    pub models: Arc<dyn SessionRunnerModel>,
    pub context: Arc<dyn SystemContextLoader>,
    pub compaction: Arc<dyn Compaction>,
    pub snapshots: Arc<dyn Snapshots>,
    pub location: LocationRef,
}

impl LlmSessionRunner {
    pub fn new(deps: RunnerDeps) -> Self {
        Self {
            store: SessionStore::new(deps.events.database()),
            events: deps.events,
            llm: deps.llm,
            agents: deps.agents,
            tools: deps.tools,
            models: deps.models,
            context: deps.context,
            compaction: deps.compaction,
            snapshots: deps.snapshots,
            location: deps.location,
        }
    }

    /// `SessionRunner.run` with an explicit interruption token.
    pub async fn drain(
        &self,
        session_id: &SessionID,
        force: bool,
        cancel: &CancellationToken,
    ) -> Result<(), SessionError> {
        let has_steer = self.has_pending(session_id, Delivery::Steer).await?;
        let has_queue = !has_steer && self.has_pending(session_id, Delivery::Queue).await?;
        if !force && !has_steer && !has_queue {
            return Ok(());
        }
        self.fail_interrupted_tools(session_id).await?;
        let mut promotion = match (has_steer, has_queue) {
            (true, _) => Some(Delivery::Steer),
            (_, true) => Some(Delivery::Queue),
            _ => None,
        };
        let mut should_run = true;
        while should_run {
            let mut needs_continuation = true;
            let mut step = 1;
            while needs_continuation {
                let (continuation, result_step) =
                    self.run_turn(session_id, promotion, step, cancel).await?;
                needs_continuation = continuation;
                step = result_step + 1;
                promotion = Some(Delivery::Steer);
                if !needs_continuation {
                    needs_continuation = self.has_pending(session_id, Delivery::Steer).await?;
                }
            }
            should_run = self.has_pending(session_id, Delivery::Queue).await?;
            promotion = should_run.then_some(Delivery::Queue);
        }
        Ok(())
    }

    /// `runTurn`: overflow recovery enabled; transitions loop instead of recursing.
    async fn run_turn(
        &self,
        session_id: &SessionID,
        promotion: Option<Delivery>,
        step: u64,
        cancel: &CancellationToken,
    ) -> Result<(bool, u64), SessionError> {
        let mut promotion = promotion;
        let mut step = step;
        let mut recover_overflow = true;
        loop {
            match self
                .attempt(session_id, promotion, step, recover_overflow, cancel)
                .await?
            {
                Attempt::Done {
                    needs_continuation,
                    step,
                } => return Ok((needs_continuation, step)),
                Attempt::ContinueAfterCompaction { step: next } => step = next,
                Attempt::ContinueAfterOverflowCompaction { step: next } => {
                    if !recover_overflow {
                        return Err(SessionError::Run {
                            message:
                                "Post-compaction provider attempt cannot recover another overflow"
                                    .into(),
                            retryable: false,
                        });
                    }
                    recover_overflow = false;
                    step = next;
                }
            }
            promotion = None;
            tokio::task::yield_now().await;
        }
    }

    pub(crate) async fn session(
        &self,
        session_id: &SessionID,
    ) -> Result<SessionInfo, SessionError> {
        self.store
            .get(session_id)
            .await?
            .ok_or_else(|| SessionError::Run {
                message: format!("Session not found: {session_id}"),
                retryable: false,
            })
    }

    async fn has_pending(
        &self,
        session_id: &SessionID,
        delivery: Delivery,
    ) -> Result<bool, SessionError> {
        let id = session_id.clone();
        self.events
            .database()
            .read(move |sql: &dyn Sql| {
                Ok::<_, opencode_storage_api::StorageError>(inbox::has_pending(sql, &id, delivery))
            })
            .await
            .map_err(crate::projection::storage_error)?
    }

    /// `failInterruptedTools`: pending or running tools left by a previous owner fail first.
    async fn fail_interrupted_tools(&self, session_id: &SessionID) -> Result<(), SessionError> {
        for message in self.store.context(session_id).await? {
            let Message::Assistant(message) = message else {
                continue;
            };
            for content in &message.content {
                let AssistantContent::Tool(tool) = content else {
                    continue;
                };
                if !matches!(tool.state, ToolState::Pending(_) | ToolState::Running(_)) {
                    continue;
                }
                let provider = tool.provider.as_ref();
                publish_session_event(
                    self.events.as_ref(),
                    &ToolFailed {
                        timestamp: DateTimeUtc::now(),
                        session_id: session_id.clone(),
                        assistant_message_id: message.id.clone(),
                        call_id: tool.id.clone(),
                        error: UnknownError::new("Tool execution interrupted"),
                        result: None,
                        provider: ToolProvider {
                            executed: provider.is_some_and(|p| p.executed),
                            metadata: provider.and_then(|p| p.metadata.clone()),
                        },
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SessionRunner for LlmSessionRunner {
    async fn run(&self, session_id: &SessionID, force: bool) -> Result<(), SessionError> {
        self.drain(session_id, force, &CancellationToken::new())
            .await
    }

    async fn run_cancellable(
        &self,
        session_id: &SessionID,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), SessionError> {
        self.drain(session_id, force, &cancel).await
    }
}

/// Maps context-crate failures onto the Session error vocabulary (`RunError`).
pub(crate) fn context_error(session_id: &SessionID, error: ContextError) -> SessionError {
    match error {
        ContextError::SnapshotDecode { details, .. } => SessionError::ContextSnapshotDecode {
            session_id: session_id.clone(),
            details,
        },
        ContextError::InitializationBlocked { .. } => {
            SessionError::ContextInitializationBlocked(error.to_string())
        }
        ContextError::Storage(error) => crate::projection::storage_error(error),
        ContextError::Event(error) => crate::projection::event_error(error),
        other => SessionError::Run {
            message: other.to_string(),
            retryable: false,
        },
    }
}
