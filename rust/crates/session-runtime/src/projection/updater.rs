//! Purpose: fold durable Session events into `session_message` rows, porting
//! packages/core/src/session/message-updater.ts `update` and the SQL adapter of session/projector.ts `run`.
//! Scope: synchronous, runs inside the event publish transaction.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/projection/adapter.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::from_value
//! |-- sm::AgentSwitched
//! |-- sm::AgentSwitchedTag
//! |-- sm::AssistantTag
//! |-- sm::Compaction
//! |-- sm::CompactionTag
//! |-- sm::CompletedTag
//! |-- sm::ErrorTag
//! |-- sm::ModelSwitched
//! |-- sm::ModelSwitchedTag
//! |-- sm::PendingTag
//! |-- sm::ReasoningTag
//! |-- sm::RunningTag
//! |-- sm::Shell
//! |-- sm::ShellTag
//! |-- sm::Synthetic
//! |-- sm::SyntheticTag
//! |-- sm::System
//! |-- sm::SystemTag
//! |-- sm::TextTag
//! |-- sm::ToolStateCompleted
//! |-- sm::ToolStateError
//! |-- sm::ToolStatePending
//! |-- sm::ToolStateRunning
//! |-- sm::ToolTag
//! |-- sm::User
//! `-- sm::UserTag
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/projection.rs
//! Invariants: a newer turn supersedes stale incomplete assistant rows (only the latest assistant
//! row is resumed). Appended rows take the event's durable seq. Unknown or live-only event types are
//! ignored like the baseline `Effect.void` arms.
//! Contract tests: session-runtime/tests/admission_projection.rs

use opencode_event_api::Payload;
use opencode_schema::session_event::{
    AgentSwitched, CompactionEnded, ContextUpdated, ModelSwitched, Prompted, ReasoningEnded,
    ReasoningStarted, ShellEnded, ShellStarted, StepEnded, StepFailed, StepStarted, Synthetic,
    TextEnded, TextStarted, ToolCalled, ToolFailed, ToolInputEnded, ToolInputStarted, ToolProgress,
    ToolSuccess,
};
use opencode_schema::session_message::{
    self as sm, Assistant, AssistantContent, AssistantReasoning, AssistantSnapshot, AssistantText,
    AssistantTool, AssistantToolProvider, AssistantToolTime, Created, CreatedCompleted, Message,
    ToolState,
};
use opencode_schema::values::JsonObject;
use opencode_storage_api::Sql;
use serde::de::DeserializeOwned;

use super::ProjectionError;
use super::adapter::SqlAdapter;

/// Applies one durable Session event to the transcript (`SessionMessageUpdater.update`).
pub(crate) fn apply(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let adapter = SqlAdapter::new(tx, payload)?;
    let metadata = payload.metadata.clone();
    match payload.event_type.as_str() {
        "session.next.agent.switched" => {
            let e: AgentSwitched = data(payload)?;
            adapter.append(Message::AgentSwitched(sm::AgentSwitched {
                id: e.message_id,
                metadata,
                time: Created {
                    created: e.timestamp,
                },
                r#type: sm::AgentSwitchedTag,
                agent: e.agent,
            }))
        }
        "session.next.model.switched" => {
            let e: ModelSwitched = data(payload)?;
            adapter.append(Message::ModelSwitched(sm::ModelSwitched {
                id: e.message_id,
                metadata,
                time: Created {
                    created: e.timestamp,
                },
                r#type: sm::ModelSwitchedTag,
                model: e.model,
            }))
        }
        "session.next.prompted" => {
            let e: Prompted = data(payload)?;
            adapter.append(Message::User(sm::User {
                id: e.message_id,
                metadata,
                time: Created {
                    created: e.timestamp,
                },
                text: e.prompt.text,
                files: e.prompt.files,
                agents: e.prompt.agents,
                r#type: sm::UserTag,
            }))
        }
        "session.next.context.updated" => {
            let e: ContextUpdated = data(payload)?;
            adapter.append(Message::System(sm::System {
                id: e.message_id,
                metadata: None,
                time: Created {
                    created: e.timestamp,
                },
                r#type: sm::SystemTag,
                text: e.text,
            }))
        }
        "session.next.synthetic" => {
            let e: Synthetic = data(payload)?;
            adapter.append(Message::Synthetic(sm::Synthetic {
                id: e.message_id,
                metadata: None,
                time: Created {
                    created: e.timestamp,
                },
                session_id: e.session_id,
                text: e.text,
                r#type: sm::SyntheticTag,
            }))
        }
        "session.next.shell.started" => {
            let e: ShellStarted = data(payload)?;
            adapter.append(Message::Shell(sm::Shell {
                id: e.message_id,
                metadata,
                time: CreatedCompleted {
                    created: e.timestamp,
                    completed: None,
                },
                r#type: sm::ShellTag,
                call_id: e.call_id,
                command: e.command,
                output: String::new(),
            }))
        }
        "session.next.shell.ended" => {
            let e: ShellEnded = data(payload)?;
            let Some(mut shell) = adapter.current_shell(&e.call_id)? else {
                return Ok(());
            };
            shell.output = e.output;
            shell.time.completed = Some(e.timestamp);
            adapter.update(&Message::Shell(shell))
        }
        "session.next.step.started" => {
            let e: StepStarted = data(payload)?;
            if let Some(mut current) = adapter.current_assistant()? {
                current.time.completed = Some(e.timestamp);
                adapter.update(&Message::Assistant(current))?;
            }
            adapter.append(Message::Assistant(Assistant {
                id: e.assistant_message_id,
                metadata: None,
                time: CreatedCompleted {
                    created: e.timestamp,
                    completed: None,
                },
                r#type: sm::AssistantTag,
                agent: e.agent,
                model: e.model,
                content: Vec::new(),
                snapshot: e.snapshot.map(|start| AssistantSnapshot {
                    start: Some(start),
                    end: None,
                    files: None,
                }),
                finish: None,
                cost: None,
                tokens: None,
                error: None,
            }))
        }
        "session.next.step.ended" => {
            let e: StepEnded = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                draft.time.completed = Some(e.timestamp);
                draft.finish = Some(e.finish);
                draft.cost = Some(e.cost);
                draft.tokens = Some(e.tokens);
                if e.snapshot.is_some() || e.files.is_some() {
                    let previous = draft.snapshot.take();
                    draft.snapshot = Some(AssistantSnapshot {
                        start: previous.and_then(|snapshot| snapshot.start),
                        end: e.snapshot,
                        files: e.files,
                    });
                }
            })
        }
        "session.next.step.failed" => {
            let e: StepFailed = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                draft.time.completed = Some(e.timestamp);
                draft.finish = Some("error".into());
                draft.error = Some(e.error);
            })
        }
        "session.next.text.started" => {
            let e: TextStarted = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                draft.content.push(AssistantContent::Text(AssistantText {
                    r#type: sm::TextTag,
                    id: e.text_id,
                    text: String::new(),
                }));
            })
        }
        "session.next.text.ended" => {
            let e: TextEnded = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                let found = draft.content.iter_mut().rev().find_map(|item| match item {
                    AssistantContent::Text(text) if text.id == e.text_id => Some(text),
                    _ => None,
                });
                if let Some(text) = found {
                    text.text = e.text;
                }
            })
        }
        "session.next.tool.input.started" => {
            let e: ToolInputStarted = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                draft.content.push(AssistantContent::Tool(AssistantTool {
                    r#type: sm::ToolTag,
                    id: e.call_id,
                    name: e.name,
                    provider: None,
                    state: ToolState::Pending(sm::ToolStatePending {
                        status: sm::PendingTag,
                        input: String::new(),
                    }),
                    time: AssistantToolTime {
                        created: e.timestamp,
                        ran: None,
                        completed: None,
                        pruned: None,
                    },
                }));
            })
        }
        "session.next.tool.input.ended" => {
            let e: ToolInputEnded = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                if let Some(tool) = latest_tool(draft, &e.call_id)
                    && let ToolState::Pending(pending) = &mut tool.state
                {
                    pending.input = e.text;
                }
            })
        }
        "session.next.tool.called" => {
            let e: ToolCalled = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                if let Some(tool) = latest_tool(draft, &e.call_id) {
                    tool.provider = Some(AssistantToolProvider {
                        executed: e.provider.executed,
                        metadata: e.provider.metadata,
                        result_metadata: None,
                    });
                    tool.time.ran = Some(e.timestamp);
                    tool.state = ToolState::Running(sm::ToolStateRunning {
                        status: sm::RunningTag,
                        input: e.input,
                        structured: JsonObject::new(),
                        content: Vec::new(),
                    });
                }
            })
        }
        "session.next.tool.progress" => {
            let e: ToolProgress = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                if let Some(tool) = latest_tool(draft, &e.call_id)
                    && let ToolState::Running(running) = &mut tool.state
                {
                    running.structured = e.structured;
                    running.content = e.content;
                }
            })
        }
        "session.next.tool.success" => {
            let e: ToolSuccess = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                let Some(tool) = latest_tool(draft, &e.call_id) else {
                    return;
                };
                let ToolState::Running(running) = &tool.state else {
                    return;
                };
                let input = running.input.clone();
                tool.provider = Some(settled_provider(
                    tool.provider.take(),
                    e.provider.executed,
                    e.provider.metadata,
                ));
                tool.time.completed = Some(e.timestamp);
                tool.state = ToolState::Completed(sm::ToolStateCompleted {
                    status: sm::CompletedTag,
                    input,
                    attachments: None,
                    content: e.content,
                    output_paths: Some(e.output_paths.unwrap_or_default()),
                    structured: e.structured,
                    result: e.result,
                });
            })
        }
        "session.next.tool.failed" => {
            let e: ToolFailed = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                let Some(tool) = latest_tool(draft, &e.call_id) else {
                    return;
                };
                let (input, structured, content) = match &tool.state {
                    ToolState::Pending(_) => (JsonObject::new(), JsonObject::new(), Vec::new()),
                    ToolState::Running(running) => (
                        running.input.clone(),
                        running.structured.clone(),
                        running.content.clone(),
                    ),
                    _ => return,
                };
                tool.provider = Some(settled_provider(
                    tool.provider.take(),
                    e.provider.executed,
                    e.provider.metadata,
                ));
                tool.time.completed = Some(e.timestamp);
                tool.state = ToolState::Error(sm::ToolStateError {
                    status: sm::ErrorTag,
                    input,
                    content,
                    structured,
                    error: e.error,
                    result: e.result,
                });
            })
        }
        "session.next.reasoning.started" => {
            let e: ReasoningStarted = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                draft
                    .content
                    .push(AssistantContent::Reasoning(AssistantReasoning {
                        r#type: sm::ReasoningTag,
                        id: e.reasoning_id,
                        text: String::new(),
                        provider_metadata: e.provider_metadata,
                        time: Some(CreatedCompleted {
                            created: e.timestamp,
                            completed: None,
                        }),
                    }));
            })
        }
        "session.next.reasoning.ended" => {
            let e: ReasoningEnded = data(payload)?;
            adapter.owned(&e.assistant_message_id, |draft| {
                let found = draft.content.iter_mut().rev().find_map(|item| match item {
                    AssistantContent::Reasoning(reasoning) if reasoning.id == e.reasoning_id => {
                        Some(reasoning)
                    }
                    _ => None,
                });
                if let Some(reasoning) = found {
                    reasoning.text = e.text;
                    let created = reasoning
                        .time
                        .as_ref()
                        .map_or(e.timestamp, |time| time.created);
                    reasoning.time = Some(CreatedCompleted {
                        created,
                        completed: Some(e.timestamp),
                    });
                    if e.provider_metadata.is_some() {
                        reasoning.provider_metadata = e.provider_metadata;
                    }
                }
            })
        }
        "session.next.compaction.ended" => {
            let e: CompactionEnded = data(payload)?;
            adapter.append(Message::Compaction(sm::Compaction {
                r#type: sm::CompactionTag,
                reason: e.reason,
                summary: e.text,
                recent: e.recent,
                id: e.message_id,
                metadata,
                time: Created {
                    created: e.timestamp,
                },
            }))
        }
        _ => Ok(()),
    }
}

/// `{ executed: event.executed || match.provider?.executed === true, metadata: match.provider?.metadata, resultMetadata }`.
fn settled_provider(
    previous: Option<AssistantToolProvider>,
    executed: bool,
    result_metadata: Option<opencode_schema::prompt::ProviderMetadata>,
) -> AssistantToolProvider {
    let previous_executed = previous.as_ref().is_some_and(|p| p.executed);
    AssistantToolProvider {
        executed: executed || previous_executed,
        metadata: previous.and_then(|p| p.metadata),
        result_metadata,
    }
}

fn latest_tool<'a>(draft: &'a mut Assistant, call_id: &str) -> Option<&'a mut AssistantTool> {
    draft.content.iter_mut().rev().find_map(|item| match item {
        AssistantContent::Tool(tool) if tool.id == call_id => Some(tool),
        _ => None,
    })
}

fn data<T: DeserializeOwned>(payload: &Payload) -> Result<T, ProjectionError> {
    serde_json::from_value(payload.data.clone()).map_err(|error| ProjectionError::Decode {
        event_type: payload.event_type.clone(),
        message: error.to_string(),
    })
}
