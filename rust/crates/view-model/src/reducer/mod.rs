//! Purpose: canonical V2 event-to-source reducer. Port of
//! `packages/app/src/context/server-session-v2-reducer.ts:14-509`: admission/promotion,
//! step/text/reasoning/tool/retry folding, shell and compaction lifecycle.
//! Scope: pure state transition per event, plus the per-reducer pending-admission map.
//! Uses:
//! rust/crates/
//! |-- view-model/src/reducer/assistant.rs
//! `-- view-model/src/source.rs
//! External:
//! `-- serde_json::{Value, from_value}
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! |-- view-model/src/reducer/assistant.rs
//! `-- view-model/tests/reducer.rs
//! Invariants: events without a string `data.sessionID` or with unknown types return `None`.
//! Appends are idempotent by message id. Updates to unknown targets touch nothing. A promotion
//! without a recorded admission reports `missing` so the caller hydrates from the server.
//! Contract tests: view-model/tests/reducer.rs

mod assistant;

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::source::{
    CompactionState, CreatedTime, PendingInput, SessionEvent, SessionMessage, ShellOutput,
    SpanTime, StructuredError,
};
use assistant::update_assistant;

/// Result of folding one event, `V2SessionReduction` in the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct Reduction {
    pub session_id: String,
    pub messages: Vec<SessionMessage>,
    /// Message ids whose content changed (drives projection invalidation).
    pub touched: Vec<String>,
    /// Promoted input id whose admission was never seen; caller must hydrate.
    pub missing: Option<String>,
}

/// Stateful reducer: remembers admitted inputs until promotion, keyed by session and input.
#[derive(Debug, Default)]
pub struct V2SessionReducer {
    pending: HashMap<(String, String), PendingInput>,
}

pub(super) fn field<T: DeserializeOwned>(data: &Value, key: &str) -> Option<T> {
    data.get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub(super) fn string(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn message_id(event_id: &str) -> String {
    match event_id.strip_prefix("evt_") {
        Some(rest) => format!("msg_{rest}"),
        None => event_id.to_owned(),
    }
}

impl V2SessionReducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop pending admissions of one session (session reset or unload).
    pub fn clear(&mut self, session_id: &str) {
        self.pending.retain(|(sid, _), _| sid != session_id);
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn reduce(&mut self, source: &[SessionMessage], event: &SessionEvent) -> Option<Reduction> {
        let data = &event.data;
        let session_id = data.get("sessionID")?.as_str()?.to_owned();
        let result = |messages: Vec<SessionMessage>, touched: Vec<String>| Reduction {
            session_id: session_id.clone(),
            messages,
            touched,
            missing: None,
        };
        let append = |message: SessionMessage| {
            let id = message.id().to_owned();
            let mut messages = source.to_vec();
            if !source.iter().any(|m| m.id() == id) {
                messages.push(message);
            }
            result(messages, vec![id])
        };
        let created = event.created;

        match event.kind.as_str() {
            "session.input.admitted" => {
                let input: PendingInput = field(data, "input")?;
                self.pending
                    .insert((session_id.clone(), string(data, "inputID")), input);
                Some(result(source.to_vec(), vec![]))
            }
            "session.input.promoted" => {
                let input_id = string(data, "inputID");
                let Some(input) = self.pending.remove(&(session_id.clone(), input_id.clone()))
                else {
                    let mut r = result(source.to_vec(), vec![]);
                    r.missing = Some(input_id);
                    return Some(r);
                };
                let time = CreatedTime { created };
                if input.kind == "user" {
                    return Some(append(SessionMessage::User {
                        id: input_id,
                        metadata: input.data.metadata,
                        time,
                        text: input.data.text,
                        files: input.data.files,
                        agents: input.data.agents,
                    }));
                }
                Some(append(SessionMessage::Synthetic {
                    id: input_id,
                    metadata: input.data.metadata,
                    time,
                    text: input.data.text,
                    description: input.data.description,
                }))
            }
            "session.agent.selected" => Some(append(SessionMessage::AgentSwitched {
                id: message_id(&event.id),
                metadata: event.metadata.clone(),
                time: CreatedTime { created },
                agent: string(data, "agent"),
            })),
            "session.model.selected" => {
                let previous = source.iter().rev().find_map(|m| match m {
                    SessionMessage::ModelSwitched { model, .. } => Some(model.clone()),
                    SessionMessage::Assistant(a) => Some(a.model.clone()),
                    _ => None,
                });
                Some(append(SessionMessage::ModelSwitched {
                    id: message_id(&event.id),
                    metadata: event.metadata.clone(),
                    time: CreatedTime { created },
                    model: field(data, "model")?,
                    previous,
                }))
            }
            "session.synthetic" => Some(append(SessionMessage::Synthetic {
                id: message_id(&event.id),
                metadata: field(data, "metadata"),
                time: CreatedTime { created },
                text: string(data, "text"),
                description: field(data, "description"),
            })),
            "session.skill.activated" => Some(append(SessionMessage::Skill {
                id: message_id(&event.id),
                metadata: event.metadata.clone(),
                time: CreatedTime { created },
                skill: string(data, "id"),
                name: string(data, "name"),
                text: string(data, "text"),
            })),
            "session.shell.started" => {
                let shell = data.get("shell")?;
                Some(append(SessionMessage::Shell {
                    id: message_id(&event.id),
                    metadata: event.metadata.clone(),
                    time: SpanTime {
                        created,
                        completed: None,
                    },
                    shell_id: string(shell, "id"),
                    command: string(shell, "command"),
                    status: string(shell, "status"),
                    exit: shell.get("exit").cloned(),
                    output: None,
                }))
            }
            "session.shell.ended" => {
                let shell = data.get("shell")?;
                let shell_id = string(shell, "id");
                let output: Option<ShellOutput> = field(data, "output");
                let index = source.iter().rposition(
                    |m| matches!(m, SessionMessage::Shell { shell_id: s, .. } if *s == shell_id),
                );
                Some(update_at(source, index, &session_id, |m| {
                    if let SessionMessage::Shell {
                        status,
                        exit,
                        output: out,
                        time,
                        ..
                    } = m
                    {
                        *status = string(shell, "status");
                        *exit = shell.get("exit").cloned();
                        *out = output.clone();
                        time.completed = Some(created);
                    }
                }))
            }
            kind if is_assistant_scoped(kind) => assistant::reduce(source, event, &session_id),
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted" => {
                let current = source.iter().rev().find_map(|m| match m {
                    SessionMessage::Assistant(a) if a.time.completed.is_none() => Some(a),
                    _ => None,
                });
                match current {
                    Some(a) if a.retry.is_some() => {
                        let id = a.id.clone();
                        Some(update_assistant(source, &id, &session_id, |a| {
                            a.retry = None
                        }))
                    }
                    _ => Some(result(source.to_vec(), vec![])),
                }
            }
            "session.compaction.started" => Some(append(SessionMessage::Compaction {
                id: field(data, "inputID").unwrap_or_else(|| message_id(&event.id)),
                metadata: event.metadata.clone(),
                time: CreatedTime { created },
                reason: string(data, "reason"),
                state: CompactionState::Running {
                    summary: String::new(),
                    recent: string(data, "recent"),
                },
            })),
            "session.compaction.delta" => {
                let index = running_compaction(source);
                let text = string(data, "text");
                Some(update_at(source, index, &session_id, |m| {
                    if let SessionMessage::Compaction {
                        state: CompactionState::Running { summary, .. },
                        ..
                    } = m
                    {
                        summary.push_str(&text);
                    }
                }))
            }
            "session.compaction.ended" => {
                let completed = CompactionState::Completed {
                    summary: string(data, "text"),
                    recent: string(data, "recent"),
                };
                let Some(index) = running_compaction(source) else {
                    return Some(append(SessionMessage::Compaction {
                        id: message_id(&event.id),
                        metadata: event.metadata.clone(),
                        time: CreatedTime { created },
                        reason: string(data, "reason"),
                        state: completed,
                    }));
                };
                Some(update_at(source, Some(index), &session_id, |m| {
                    if let SessionMessage::Compaction { reason, state, .. } = m {
                        *reason = string(data, "reason");
                        *state = completed.clone();
                    }
                }))
            }
            "session.compaction.failed" => {
                let failed = CompactionState::Failed {
                    error: field::<StructuredError>(data, "error")?,
                };
                let Some(index) = running_compaction(source) else {
                    return Some(append(SessionMessage::Compaction {
                        id: field(data, "inputID").unwrap_or_else(|| message_id(&event.id)),
                        metadata: event.metadata.clone(),
                        time: CreatedTime { created },
                        reason: string(data, "reason"),
                        state: failed,
                    }));
                };
                Some(update_at(source, Some(index), &session_id, |m| {
                    if let SessionMessage::Compaction { reason, state, .. } = m {
                        *reason = string(data, "reason");
                        *state = failed.clone();
                    }
                }))
            }
            _ => None,
        }
    }
}

fn is_assistant_scoped(kind: &str) -> bool {
    kind.starts_with("session.step.")
        || kind.starts_with("session.text.")
        || kind.starts_with("session.reasoning.")
        || kind.starts_with("session.tool.")
        || kind == "session.retry.scheduled"
}

fn running_compaction(source: &[SessionMessage]) -> Option<usize> {
    source.iter().rposition(|m| {
        matches!(
            m,
            SessionMessage::Compaction {
                state: CompactionState::Running { .. },
                ..
            }
        )
    })
}

fn update_at(
    source: &[SessionMessage],
    index: Option<usize>,
    session_id: &str,
    apply: impl FnOnce(&mut SessionMessage),
) -> Reduction {
    let mut messages = source.to_vec();
    let touched = match index {
        Some(i) => {
            apply(&mut messages[i]);
            vec![messages[i].id().to_owned()]
        }
        None => vec![],
    };
    Reduction {
        session_id: session_id.to_owned(),
        messages,
        touched,
        missing: None,
    }
}
