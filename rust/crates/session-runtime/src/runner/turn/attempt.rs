//! Purpose: llm.ts `runTurnAttempt`: one provider attempt. Selects the agent, initializes or
//! prepares the context epoch, promotes steers/queue inputs, reloads runner history, builds the
//! request, makes exactly one `llm.stream(request)` call, publishes events, settles local tool calls
//! concurrently, and applies the baseline failure/interrupt/step-ended settlement order.
//! Scope: one provider turn. Transitions (compaction) are returned to `turn.rs`.
//! Uses:
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- llm-api/src/lib.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/max_steps.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/src/runner/turn/request.rs
//! |-- session-runtime/src/runner/turn/tools.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- epoch::initialize
//! |-- epoch::latest_sequence
//! |-- epoch::prepare
//! |-- futures::StreamExt
//! |-- inbox::promote_next_queued
//! |-- inbox::promote_steers
//! |-- request::RequestInput
//! |-- request::build
//! |-- request::entries
//! |-- tokio::select
//! |-- tokio::sync::Mutex
//! |-- tokio_util::sync::CancellationToken
//! `-- usize::from
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/runner/turn.rs
//! Invariants: one `stream` call per attempt; tool calls are published before settlement starts;
//! provider-executed calls are never settled locally; every spawned settlement is joined before
//! the attempt returns; a context-overflow provider error before the assistant starts is withheld
//! so compaction can retry; cancellation fails unsettled tools and the active assistant.
//! Contract tests: session-runtime/tests/runner_turn.rs, session-runtime/tests/runner_turn_tools.rs

use std::sync::Arc;

use futures::StreamExt;
use opencode_context::epoch;
use opencode_llm_api::{LlmError, LlmEvent, ProviderFailureClassification};
use opencode_schema::provider::ModelRef;
use opencode_schema::session_event::StepEnded;
use opencode_schema::values::{DateTimeUtc, Finite};
use opencode_schema::{ModelID, ProviderID};
use opencode_session_api::{Delivery, SessionError, SessionID};
use opencode_storage_api::{DatabaseExt, Sql};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::tools::{ToolTasks, TurnFailure};
use super::{LlmSessionRunner, context_error, request};
use crate::inbox;
use crate::projection::publish_session_event;
use crate::runner::max_steps::is_last_step;
use crate::runner::ports::CompactionInput;
use crate::runner::publish::{LlmEventPublisher, PublisherInput};

/// Result of one attempt (`runTurnAttempt` success value or `TurnTransitionError`).
pub(super) enum Attempt {
    Done {
        needs_continuation: bool,
        step: u64,
    },
    /// Automatic compaction completed; rebuild the request from compacted history.
    ContinueAfterCompaction {
        step: u64,
    },
    /// Overflow compaction completed; rebuild once through the path without overflow recovery.
    ContinueAfterOverflowCompaction {
        step: u64,
    },
}

impl LlmSessionRunner {
    pub(super) async fn attempt(
        &self,
        session_id: &SessionID,
        promotion: Option<Delivery>,
        step: u64,
        recover_overflow: bool,
        cancel: &CancellationToken,
    ) -> Result<Attempt, SessionError> {
        let session = self.session(session_id).await?;
        if session.location != self.location {
            return Err(SessionError::Cancelled);
        }
        let agent = self
            .agents
            .select(session.agent.as_ref().map(|id| id.as_str()));
        let db = self.events.database();
        let initialized =
            epoch::initialize(db.as_ref(), self.context.load(&agent), session_id.as_str())
                .await
                .map_err(|error| context_error(session_id, error))?;
        let mut current_step = step;
        if let Some(promotion) = promotion {
            let id = session_id.as_str().to_owned();
            let cutoff = db
                .read(move |sql: &dyn Sql| epoch::latest_sequence(sql, &id))
                .await
                .map_err(|error| context_error(session_id, error))?;
            let mut promoted = 0;
            if promotion == Delivery::Queue {
                promoted += usize::from(
                    inbox::promote_next_queued(self.events.as_ref(), session_id).await?,
                );
            }
            promoted += inbox::promote_steers(self.events.as_ref(), session_id, cutoff).await?;
            if promoted > 0 {
                current_step = 1;
            }
        }
        let system = match initialized {
            Some(prepared) => prepared,
            None => {
                let store = self.events.store();
                epoch::prepare(
                    db.as_ref(),
                    store.as_ref(),
                    self.context.load(&agent),
                    session_id.as_str(),
                )
                .await
                .map_err(|error| context_error(session_id, error))?
            }
        };
        let model = self.models.resolve(&session).await?;
        let entries =
            request::entries(self.events.as_ref(), session_id, system.baseline_seq).await?;
        let context: Vec<_> = entries.iter().map(|(_, message)| message.clone()).collect();
        let info = agent.info.as_ref();
        let last = is_last_step(
            info.and_then(|i| i.steps).map(|s| s.get() as u64),
            current_step,
        );
        let tools = (!last).then(|| {
            self.tools
                .materialize(info.map_or(&[][..], |i| &i.permissions))
        });
        let request = request::build(request::RequestInput {
            session: &session,
            model: model.clone(),
            agent_system: info.and_then(|i| i.system.as_deref()),
            baseline: &system.baseline,
            context: &context,
            is_last_step: last,
            tools: tools.as_ref().map(|t| t.definitions()).unwrap_or_default(),
        });
        let compaction = CompactionInput {
            session_id,
            entries: &entries,
            model: &model,
            request: &request,
        };
        let compacted = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SessionError::Cancelled),
            result = self.compaction.compact_if_needed(&compaction) => result?,
        };
        if compacted {
            return Ok(Attempt::ContinueAfterCompaction { step: current_step });
        }

        let start_snapshot = self.snapshots.capture().await;
        let publisher = Arc::new(Mutex::new(LlmEventPublisher::new(
            self.events.clone(),
            PublisherInput {
                session_id: session_id.clone(),
                agent: agent.id.as_str().to_owned(),
                model: ModelRef {
                    id: ModelID::new(model.id.as_str()),
                    provider_id: ProviderID::new(model.provider.as_str()),
                    variant: session.model.as_ref().and_then(|m| m.variant.clone()),
                },
                snapshot: start_snapshot.clone(),
            },
        )));
        let mut tasks = ToolTasks::new(publisher.clone(), cancel.child_token());
        let mut needs_continuation = false;
        let mut overflow: Option<LlmEvent> = None;
        let mut failure: Option<LlmError> = None;
        let mut interrupted = false;
        let mut defect: Option<SessionError> = None;

        // Exactly one provider call per attempt.
        let mut stream = self.llm.stream(request.clone());
        loop {
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => { interrupted = true; break; }
                next = stream.next() => next,
            };
            let event = match next {
                None => break,
                Some(Err(error)) => {
                    failure = Some(error);
                    break;
                }
                Some(Ok(event)) => event,
            };
            let mut guard = publisher.lock().await;
            if overflow.is_some() || guard.has_provider_error() {
                continue;
            }
            if is_overflow_event(&event) && !guard.has_assistant_started() {
                overflow = Some(event);
                continue;
            }
            if let Err(error) = guard.publish(&event, &[]).await {
                defect = Some(error.into());
                break;
            }
            let LlmEvent::ToolCall {
                id,
                name,
                input,
                provider_executed,
                ..
            } = &event
            else {
                continue;
            };
            if *provider_executed == Some(true) {
                continue;
            }
            let Some(materialized) = &tools else {
                let disabled = "Tools are disabled after the maximum agent steps";
                if let Err(error) = guard.fail_unsettled_tools(disabled, false).await {
                    defect = Some(error.into());
                    break;
                }
                continue;
            };
            needs_continuation = true;
            let assistant_message_id = match guard.assistant_message_id(id) {
                Ok(message_id) => message_id,
                Err(error) => {
                    defect = Some(error.into());
                    break;
                }
            };
            drop(guard);
            tasks.spawn(
                materialized.clone(),
                crate::runner::ports::SettleInput {
                    session_id: session_id.clone(),
                    agent: agent.id.as_str().to_owned(),
                    assistant_message_id,
                    call_id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
            );
        }
        drop(stream);
        let mut guard = publisher.lock().await;
        guard.flush().await?;
        if let Some(defect) = defect {
            drop(guard);
            tasks.cancel_all().await;
            return Err(defect);
        }

        let overflowed = overflow.is_some()
            || failure
                .as_ref()
                .is_some_and(|error| error.reason.is_context_overflow());
        if recover_overflow && !guard.has_assistant_started() && overflowed && !interrupted {
            drop(guard);
            tasks.cancel_all().await;
            let compacted = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(SessionError::Cancelled),
                result = self.compaction.compact_after_overflow(&compaction) => result?,
            };
            if compacted {
                return Ok(Attempt::ContinueAfterOverflowCompaction { step: current_step });
            }
            guard = publisher.lock().await;
        }
        if let Some(event) = &overflow {
            guard.publish(event, &[]).await?;
        }
        if let Some(error) = &failure
            && !guard.has_provider_error()
        {
            guard
                .fail_unsettled_tools("Provider did not return a tool result", true)
                .await?;
            guard.fail_assistant(&error.reason.message()).await?;
        }
        drop(guard);
        if interrupted {
            tasks.cancel_all().await;
        }
        let settled = tasks.join().await;
        let mut guard = publisher.lock().await;
        if let Err(TurnFailure::Declined) = settled {
            guard
                .fail_unsettled_tools("Tool execution interrupted", false)
                .await?;
            return Err(SessionError::Cancelled);
        }
        if interrupted || matches!(settled, Err(TurnFailure::Interrupted)) {
            guard
                .fail_unsettled_tools("Tool execution interrupted", false)
                .await?;
            if guard.has_active_assistant() {
                guard.fail_assistant("Provider turn interrupted").await?;
            }
        }
        if let Err(TurnFailure::Failed(message)) = &settled {
            let message = format!("Tool execution failed: {message}");
            guard.fail_unsettled_tools(&message, false).await?;
        }
        if let Some(settlement) = guard.step_settlement().cloned()
            && !guard.has_provider_error()
        {
            let end_snapshot = self.snapshots.capture().await;
            let files = match (&start_snapshot, &end_snapshot) {
                (Some(from), Some(to)) => self.snapshots.files(from, to).await,
                _ => None,
            };
            let assistant_message_id = guard.start_assistant().await?;
            publish_session_event(
                self.events.as_ref(),
                &StepEnded {
                    timestamp: DateTimeUtc::now(),
                    session_id: session_id.clone(),
                    assistant_message_id,
                    finish: settlement.finish,
                    cost: Finite::from(0),
                    tokens: settlement.tokens,
                    snapshot: end_snapshot,
                    files,
                },
            )
            .await?;
        }
        if guard.has_provider_error() {
            guard
                .fail_unsettled_tools("Tool execution interrupted", false)
                .await?;
        }
        let stream_succeeded = failure.is_none() && !interrupted;
        if stream_succeeded && !guard.has_provider_error() {
            guard
                .fail_unsettled_tools("Provider did not return a tool result", true)
                .await?;
        }
        if interrupted || matches!(settled, Err(TurnFailure::Interrupted)) {
            return Err(SessionError::Cancelled);
        }
        if let Some(error) = failure {
            return Err(llm_error(error));
        }
        Ok(Attempt::Done {
            needs_continuation: !guard.has_provider_error() && needs_continuation,
            step: current_step,
        })
    }
}

/// `isContextOverflowFailure` for a provider-error event.
fn is_overflow_event(event: &LlmEvent) -> bool {
    matches!(
        event,
        LlmEvent::ProviderError {
            classification: Some(ProviderFailureClassification::ContextOverflow),
            ..
        }
    )
}

/// `LLMError` surfaced by the run (`RunError`).
fn llm_error(error: LlmError) -> SessionError {
    if error.is_cancelled() {
        return SessionError::Cancelled;
    }
    SessionError::Run {
        retryable: error.retryable(),
        message: error.to_string(),
    }
}
