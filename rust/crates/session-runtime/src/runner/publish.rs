//! Purpose: publish-llm-event.ts `createLLMEventPublisher`: persist one provider turn as Session
//! step, text, reasoning and tool events without executing tools or starting a continuation turn.
//! Scope: per-turn mutable state, publishes through the `SessionEvents` facade. Callers serialize
//! access (the baseline wraps every call in a one-permit semaphore, here `&mut self`).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- llm-api/src/event.rs
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/error.rs
//! |-- session-runtime/src/projection.rs
//! `-- session-runtime/src/runner/publish/tools.rs
//! External:
//! `-- indexmap::IndexMap
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/publish/tools.rs
//! |-- session-runtime/tests/runner_publish.rs
//! |-- session-runtime/tests/runner_publish_tools.rs
//! `-- session-runtime/tests/support/publish.rs
//! Invariants: the assistant step starts lazily at most once per turn; every tool call is published
//! (`tool.called`) before any settlement; provider-executed results are published as received and
//! never require local execution; `step-finish` only records the settlement (the caller publishes
//! `step.ended`); stream protocol violations (baseline `Effect.die`) surface as
//! `PublishError::Protocol` with the baseline message.
//! Contract tests: session-runtime/tests/runner_publish.rs, session-runtime/tests/runner_publish_tools.rs

mod tools;

use std::sync::Arc;

use indexmap::IndexMap;
use opencode_event_api::{EventError, PublishOptions};
use opencode_llm_api::event::{LlmEvent, Usage};
use opencode_schema::event::EventData;
use opencode_schema::ids::{MessageID, SessionID};
use opencode_schema::prompt::ProviderMetadata;
use opencode_schema::provider::{CacheCost, ModelRef};
use opencode_schema::session_event::{
    ReasoningDelta, ReasoningStarted, StepFailed, StepStarted, TextDelta, TextStarted, ToolFailed,
    ToolInputDelta, ToolProvider,
};
use opencode_schema::session_message::{Tokens, UnknownError};
use opencode_schema::values::{DateTimeUtc, Finite};
use opencode_session_api::SessionError;

use crate::projection::{self, SessionEvents};

/// Publisher `Input`: the identity spread into `step.started`.
#[derive(Debug, Clone)]
pub struct PublisherInput {
    pub session_id: SessionID,
    pub agent: String,
    pub model: ModelRef,
    pub snapshot: Option<String>,
}

/// Recorded `step-finish`: the caller publishes `step.ended` from it.
#[derive(Debug, Clone, PartialEq)]
pub struct StepSettlement {
    pub finish: String,
    pub tokens: Tokens,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// Baseline defect (`Effect.die`): the provider stream violated the event protocol.
    #[error("{0}")]
    Protocol(String),
    #[error(transparent)]
    Event(#[from] EventError),
}

impl From<PublishError> for SessionError {
    fn from(error: PublishError) -> Self {
        match error {
            PublishError::Protocol(message) => SessionError::Run {
                message,
                retryable: false,
            },
            PublishError::Event(error) => projection::event_error(error),
        }
    }
}

/// `true` when a published event must be settled by a local tool (non-provider-executed call).
pub fn requires_local_settlement(event: &LlmEvent) -> bool {
    matches!(event, LlmEvent::ToolCall { provider_executed, .. } if *provider_executed != Some(true))
}

struct ToolEntry {
    assistant_message_id: MessageID,
    name: String,
    input_ended: bool,
    called: bool,
    settled: bool,
    provider_executed: bool,
    provider_metadata: Option<ProviderMetadata>,
}

/// Buffered stream fragments keyed by id, in start order (JS `Map` iteration order).
struct Fragments {
    name: &'static str,
    chunks: IndexMap<String, String>,
}

impl Fragments {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            chunks: IndexMap::new(),
        }
    }
    fn start(&mut self, id: &str) -> Result<(), PublishError> {
        if self.chunks.contains_key(id) {
            return Err(protocol(format!("Duplicate {} start: {id}", self.name)));
        }
        self.chunks.insert(id.to_owned(), String::new());
        Ok(())
    }
    fn append(&mut self, id: &str, value: &str) -> Result<(), PublishError> {
        let Some(current) = self.chunks.get_mut(id) else {
            return Err(protocol(format!("{} delta before start: {id}", self.name)));
        };
        current.push_str(value);
        Ok(())
    }
    fn joined(&self, id: &str) -> Result<String, PublishError> {
        self.chunks
            .get(id)
            .cloned()
            .ok_or_else(|| protocol(format!("{} end before start: {id}", self.name)))
    }
    fn ids(&self) -> Vec<String> {
        self.chunks.keys().cloned().collect()
    }
}

/// `createLLMEventPublisher(events, input)`.
pub struct LlmEventPublisher {
    events: Arc<dyn SessionEvents>,
    input: PublisherInput,
    tools: IndexMap<String, ToolEntry>,
    assistant_message_id: Option<MessageID>,
    assistant_active: bool,
    assistant_failed: bool,
    provider_failed: bool,
    step_settlement: Option<StepSettlement>,
    text: Fragments,
    reasoning: Fragments,
    tool_input: Fragments,
}

impl LlmEventPublisher {
    pub fn new(events: Arc<dyn SessionEvents>, input: PublisherInput) -> Self {
        Self {
            events,
            input,
            tools: IndexMap::new(),
            assistant_message_id: None,
            assistant_active: false,
            assistant_failed: false,
            provider_failed: false,
            step_settlement: None,
            text: Fragments::new("text"),
            reasoning: Fragments::new("reasoning"),
            tool_input: Fragments::new("tool input"),
        }
    }

    pub fn has_active_assistant(&self) -> bool {
        self.assistant_active
    }
    pub fn has_assistant_started(&self) -> bool {
        self.assistant_message_id.is_some()
    }
    pub fn has_provider_error(&self) -> bool {
        self.provider_failed
    }
    pub fn step_settlement(&self) -> Option<&StepSettlement> {
        self.step_settlement.as_ref()
    }

    /// `assistantMessageID(callID)`: the assistant message owning a known tool call.
    pub fn assistant_message_id(&self, call_id: &str) -> Result<MessageID, PublishError> {
        self.tools
            .get(call_id)
            .map(|tool| tool.assistant_message_id.clone())
            .ok_or_else(|| protocol(format!("Unknown tool call: {call_id}")))
    }

    /// `startAssistant`: publishes `step.started` once and returns the assistant message ID.
    pub async fn start_assistant(&mut self) -> Result<MessageID, PublishError> {
        if let Some(id) = &self.assistant_message_id {
            return Ok(id.clone());
        }
        let id = MessageID::create();
        self.assistant_message_id = Some(id.clone());
        self.assistant_active = true;
        self.emit(&StepStarted {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: id.clone(),
            agent: self.input.agent.clone(),
            model: self.input.model.clone(),
            snapshot: self.input.snapshot.clone(),
        })
        .await?;
        Ok(id)
    }

    /// `flush`: ends every open text, reasoning and tool-input fragment, in that order.
    pub async fn flush(&mut self) -> Result<(), PublishError> {
        for id in self.text.ids() {
            self.end_text(&id).await?;
        }
        for id in self.reasoning.ids() {
            self.end_reasoning(&id, None).await?;
        }
        for id in self.tool_input.ids() {
            self.end_tool_input_fragment(&id).await?;
        }
        Ok(())
    }

    /// `failAssistant`: flushes, ensures a step exists and publishes `step.failed` once.
    pub async fn fail_assistant(&mut self, message: &str) -> Result<(), PublishError> {
        if self.assistant_failed {
            return Ok(());
        }
        self.flush().await?;
        let id = self.start_assistant().await?;
        self.assistant_active = false;
        self.assistant_failed = true;
        self.emit(&StepFailed {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: id,
            error: UnknownError::new(message),
        })
        .await
    }

    /// `failUnsettledTools(message, hostedOnly)`: fails every unsettled (provider-executed) call.
    pub async fn fail_unsettled_tools(
        &mut self,
        message: &str,
        hosted_only: bool,
    ) -> Result<(), PublishError> {
        let call_ids: Vec<String> = self.tools.keys().cloned().collect();
        for call_id in call_ids {
            let tool = &mut self.tools[&call_id];
            if tool.settled || (hosted_only && !tool.provider_executed) {
                continue;
            }
            tool.settled = true;
            let event = ToolFailed {
                timestamp: DateTimeUtc::now(),
                session_id: self.input.session_id.clone(),
                assistant_message_id: tool.assistant_message_id.clone(),
                call_id,
                error: UnknownError::new(message),
                result: None,
                provider: ToolProvider {
                    executed: tool.provider_executed,
                    metadata: tool.provider_metadata.clone(),
                },
            };
            self.emit(&event).await?;
        }
        Ok(())
    }

    /// `publish(event, outputPaths)`: persists one LLM stream event.
    pub async fn publish(
        &mut self,
        event: &LlmEvent,
        output_paths: &[String],
    ) -> Result<(), PublishError> {
        match event {
            LlmEvent::StepStart { .. } | LlmEvent::Finish { .. } => Ok(()),
            LlmEvent::TextStart { id, .. } => {
                self.text.start(id)?;
                let assistant_message_id = self.start_assistant().await?;
                let event = TextStarted {
                    timestamp: DateTimeUtc::now(),
                    session_id: self.input.session_id.clone(),
                    assistant_message_id,
                    text_id: id.clone(),
                };
                self.emit(&event).await
            }
            LlmEvent::TextDelta { id, text, .. } => {
                self.text.append(id, text)?;
                let event = TextDelta {
                    timestamp: DateTimeUtc::now(),
                    session_id: self.input.session_id.clone(),
                    assistant_message_id: self.current_assistant()?,
                    text_id: id.clone(),
                    delta: text.clone(),
                };
                self.emit(&event).await
            }
            LlmEvent::TextEnd { id, .. } => self.end_text(id).await,
            LlmEvent::ReasoningStart {
                id,
                provider_metadata,
            } => {
                self.reasoning.start(id)?;
                let assistant_message_id = self.start_assistant().await?;
                let event = ReasoningStarted {
                    timestamp: DateTimeUtc::now(),
                    session_id: self.input.session_id.clone(),
                    assistant_message_id,
                    reasoning_id: id.clone(),
                    provider_metadata: provider_metadata.clone(),
                };
                self.emit(&event).await
            }
            LlmEvent::ReasoningDelta { id, text, .. } => {
                self.reasoning.append(id, text)?;
                let event = ReasoningDelta {
                    timestamp: DateTimeUtc::now(),
                    session_id: self.input.session_id.clone(),
                    assistant_message_id: self.current_assistant()?,
                    reasoning_id: id.clone(),
                    delta: text.clone(),
                };
                self.emit(&event).await
            }
            LlmEvent::ReasoningEnd {
                id,
                provider_metadata,
            } => self.end_reasoning(id, provider_metadata.clone()).await,
            LlmEvent::ToolInputStart { id, name, .. } => self.start_tool_input(id, name).await,
            LlmEvent::ToolInputDelta { id, name, text } => {
                let tool = self.known_tool(id, name, "Tool input delta before start", "input")?;
                if tool.input_ended {
                    return Err(protocol(format!("Tool input delta after end: {id}")));
                }
                let assistant_message_id = tool.assistant_message_id.clone();
                self.tool_input.append(id, text)?;
                let event = ToolInputDelta {
                    timestamp: DateTimeUtc::now(),
                    session_id: self.input.session_id.clone(),
                    assistant_message_id,
                    call_id: id.clone(),
                    delta: text.clone(),
                };
                self.emit(&event).await
            }
            LlmEvent::ToolInputEnd { id, name, .. } => self.end_tool_input(id, name).await,
            LlmEvent::ToolCall {
                id,
                name,
                input,
                provider_executed,
                provider_metadata,
            } => {
                self.publish_tool_call(id, name, input, *provider_executed, provider_metadata)
                    .await
            }
            LlmEvent::ToolResult {
                id,
                name,
                result,
                output,
                provider_executed,
                provider_metadata,
            } => {
                self.publish_tool_result(
                    id,
                    name,
                    result,
                    output.as_ref(),
                    *provider_executed == Some(true),
                    provider_metadata,
                    output_paths,
                )
                .await
            }
            LlmEvent::ToolError {
                id,
                name,
                message,
                provider_metadata,
                ..
            } => {
                let session_id = self.input.session_id.clone();
                let tool = self.called_tool(id, name, "error")?;
                if tool.settled {
                    return Err(protocol(format!("Duplicate tool error: {id}")));
                }
                tool.settled = true;
                let event = ToolFailed {
                    timestamp: DateTimeUtc::now(),
                    session_id,
                    assistant_message_id: tool.assistant_message_id.clone(),
                    call_id: id.clone(),
                    error: UnknownError::new(message.clone()),
                    result: None,
                    provider: ToolProvider {
                        executed: tool.provider_executed,
                        metadata: provider_metadata.clone(),
                    },
                };
                self.emit(&event).await
            }
            LlmEvent::StepFinish { reason, usage, .. } => {
                self.flush().await?;
                self.assistant_active = false;
                if self.step_settlement.is_some() {
                    return Err(protocol("Duplicate step finish".into()));
                }
                self.step_settlement = Some(StepSettlement {
                    finish: serde_json::to_value(reason)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    tokens: tokens(usage.as_ref()),
                });
                Ok(())
            }
            LlmEvent::ProviderError { message, .. } => {
                self.provider_failed = true;
                self.fail_assistant(message).await
            }
        }
    }

    fn current_assistant(&self) -> Result<MessageID, PublishError> {
        self.assistant_message_id
            .clone()
            .ok_or_else(|| protocol("Tool event before assistant step start".into()))
    }

    async fn emit<T: EventData>(&self, data: &T) -> Result<(), PublishError> {
        projection::publish(self.events.as_ref(), data, PublishOptions::default()).await?;
        Ok(())
    }
}

fn protocol(message: String) -> PublishError {
    PublishError::Protocol(message)
}

/// `safe`: non-finite or missing counts become 0, negatives clamp to 0.
fn safe(value: Option<f64>) -> Finite {
    let value = value.filter(|value| value.is_finite()).unwrap_or(0.0);
    Finite::new(value.max(0.0)).unwrap_or_default()
}

/// `tokens(usage)`: provider usage mapped to Session token accounting.
fn tokens(usage: Option<&Usage>) -> Tokens {
    Tokens {
        input: safe(usage.and_then(|usage| usage.non_cached_input_tokens)),
        output: safe(usage.map(Usage::visible_output_tokens)),
        reasoning: safe(usage.and_then(|usage| usage.reasoning_tokens)),
        cache: CacheCost {
            read: safe(usage.and_then(|usage| usage.cache_read_input_tokens)),
            write: safe(usage.and_then(|usage| usage.cache_write_input_tokens)),
        },
    }
}
