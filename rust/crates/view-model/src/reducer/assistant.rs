//! Purpose: assistant-scoped event folding for the canonical reducer: step lifecycle,
//! text/reasoning ordinals, tool call state machine and retry. Port of
//! `packages/app/src/context/server-session-v2-reducer.ts:118-335,436-509`.
//! Scope: pure functions over a source message slice.
//! Uses:
//! rust/crates/
//! |-- view-model/src/reducer/mod.rs
//! `-- view-model/src/source.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- view-model/src/reducer/mod.rs
//! Invariants: updates to an unknown assistant id touch nothing; text/reasoning addressing is
//! by per-kind ordinal; tool success only applies to running tools, failure to streaming/running.
//! Contract tests: view-model/tests/reducer.rs

use serde_json::Value;

use super::{Reduction, field, string};
use crate::source::{
    Assistant, AssistantContent, JsonObject, ModelRef, Retry, SessionEvent, SessionMessage,
    Snapshot, SpanTime, StructuredError, TokenUsage, ToolContent, ToolPart, ToolState, ToolTime,
};

/// Fold an assistant-scoped event, `None` when the type is not assistant-scoped.
pub(super) fn reduce(
    source: &[SessionMessage],
    event: &SessionEvent,
    session_id: &str,
) -> Option<Reduction> {
    let data = &event.data;
    let created = event.created;
    let assistant_id = string(data, "assistantMessageID");
    match event.kind.as_str() {
        "session.step.started" => Some(step_started(source, event, session_id)),
        "session.step.ended" => {
            let snap = step_snapshot(data);
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                a.finish = field(data, "finish");
                a.cost = field(data, "cost");
                a.tokens = field(data, "tokens");
                if let Some((end, files)) = snap.clone() {
                    a.snapshot = Some(Snapshot {
                        start: a.snapshot.take().and_then(|s| s.start),
                        end,
                        files,
                    });
                }
                a.time.completed = Some(created);
            }))
        }
        "session.step.failed" => {
            let snap = step_snapshot(data);
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                a.finish = Some("error".into());
                a.error = field(data, "error");
                a.retry = None;
                if let Some(cost) = field::<f64>(data, "cost") {
                    a.cost = Some(cost);
                }
                if let Some(tokens) = field::<TokenUsage>(data, "tokens") {
                    a.tokens = Some(tokens);
                }
                if let Some((end, files)) = snap.clone() {
                    a.snapshot = Some(Snapshot {
                        start: a.snapshot.take().and_then(|s| s.start),
                        end,
                        files,
                    });
                }
                a.time.completed = Some(created);
            }))
        }
        "session.text.started" => {
            let ordinal = ordinal(data);
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                insert_ordinal(
                    &mut a.content,
                    "text",
                    ordinal,
                    AssistantContent::Text {
                        text: String::new(),
                    },
                )
            }))
        }
        "session.text.delta"
        | "session.text.ended"
        | "session.reasoning.delta"
        | "session.reasoning.ended" => {
            let kind = if event.kind.starts_with("session.text") {
                "text"
            } else {
                "reasoning"
            };
            let ended = event.kind.ends_with(".ended");
            let delta = string(data, "delta");
            let full = string(data, "text");
            let state: Option<JsonObject> = field(data, "state");
            let ordinal = ordinal(data);
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                let Some(item) = a
                    .content
                    .iter_mut()
                    .filter(|c| c.kind() == kind)
                    .nth(ordinal)
                else {
                    return;
                };
                match item {
                    AssistantContent::Text { text } => {
                        if ended {
                            *text = full.clone()
                        } else {
                            text.push_str(&delta)
                        }
                    }
                    AssistantContent::Reasoning {
                        text,
                        state: s,
                        time,
                    } => {
                        if !ended {
                            text.push_str(&delta);
                            return;
                        }
                        *text = full.clone();
                        if state.is_some() {
                            *s = state.clone();
                        }
                        let start = time.as_ref().map(|t| t.created).unwrap_or(created);
                        *time = Some(SpanTime {
                            created: start,
                            completed: Some(created),
                        });
                    }
                    AssistantContent::Tool(_) => {}
                }
            }))
        }
        "session.reasoning.started" => {
            let ordinal = ordinal(data);
            let state: Option<JsonObject> = field(data, "state");
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                insert_ordinal(
                    &mut a.content,
                    "reasoning",
                    ordinal,
                    AssistantContent::Reasoning {
                        text: String::new(),
                        state: state.clone(),
                        time: Some(SpanTime {
                            created,
                            completed: None,
                        }),
                    },
                )
            }))
        }
        "session.tool.input.started" => {
            let call_id = string(data, "callID");
            let name = string(data, "name");
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                if a.content
                    .iter()
                    .any(|c| matches!(c, AssistantContent::Tool(t) if t.id == call_id))
                {
                    return;
                }
                a.content.push(AssistantContent::Tool(Box::new(ToolPart {
                    id: call_id.clone(),
                    name: name.clone(),
                    executed: None,
                    provider_state: None,
                    provider_result_state: None,
                    state: ToolState::Streaming {
                        input: String::new(),
                    },
                    time: ToolTime {
                        created,
                        ran: None,
                        completed: None,
                    },
                })))
            }))
        }
        kind if kind.starts_with("session.tool.") => {
            Some(tool_event(source, event, session_id, &assistant_id))
        }
        "session.retry.scheduled" => {
            let retry = Retry {
                attempt: field(data, "attempt").unwrap_or_default(),
                at: field(data, "at").unwrap_or_default(),
                error: field(data, "error")?,
            };
            Some(update_assistant(source, &assistant_id, session_id, |a| {
                a.retry = Some(retry.clone())
            }))
        }
        _ => None,
    }
}

fn step_started(source: &[SessionMessage], event: &SessionEvent, session_id: &str) -> Reduction {
    let data = &event.data;
    let created = event.created;
    let target = string(data, "assistantMessageID");
    let agent = string(data, "agent");
    let model: Option<ModelRef> = field(data, "model");
    let snapshot: Option<String> = field(data, "snapshot");
    // Complete the previous open assistant when a different step starts.
    let current = source.iter().rev().find_map(|m| match m {
        SessionMessage::Assistant(a) if a.time.completed.is_none() => Some(a.id.clone()),
        _ => None,
    });
    let mut messages = source.to_vec();
    if let Some(cur) = current.as_ref().filter(|c| **c != target) {
        for m in &mut messages {
            if let SessionMessage::Assistant(a) = m
                && a.id == *cur
            {
                a.retry = None;
                a.time.completed = Some(created);
            }
        }
    }
    let existing = messages.iter_mut().find(|m| m.id() == target);
    if let Some(SessionMessage::Assistant(a)) = existing {
        a.agent = agent;
        if let Some(model) = model {
            a.model = model;
        }
        a.retry = None;
        a.error = None;
        a.finish = None;
        if let Some(start) = snapshot {
            let mut s = a.snapshot.take().unwrap_or_default();
            s.start = Some(start);
            a.snapshot = Some(s);
        }
        a.time.completed = None;
        let touched = match current {
            Some(cur) if cur != target => vec![cur, target],
            _ => vec![target],
        };
        return Reduction {
            session_id: session_id.to_owned(),
            messages,
            touched,
            missing: None,
        };
    }
    messages.push(SessionMessage::Assistant(Assistant {
        id: target.clone(),
        metadata: event.metadata.clone(),
        time: SpanTime {
            created,
            completed: None,
        },
        agent,
        model: model.unwrap_or(ModelRef {
            id: String::new(),
            provider_id: String::new(),
            variant: None,
        }),
        content: vec![],
        snapshot: snapshot.map(|start| Snapshot {
            start: Some(start),
            end: None,
            files: None,
        }),
        finish: None,
        cost: None,
        tokens: None,
        error: None,
        retry: None,
    }));
    let touched = match current {
        Some(cur) => vec![cur, target],
        None => vec![target],
    };
    Reduction {
        session_id: session_id.to_owned(),
        messages,
        touched,
        missing: None,
    }
}

fn tool_event(
    source: &[SessionMessage],
    event: &SessionEvent,
    session_id: &str,
    assistant_id: &str,
) -> Reduction {
    let data = &event.data;
    let created = event.created;
    let call_id = string(data, "callID");
    let executed = data
        .get("executed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let metadata: Option<JsonObject> = field(data, "metadata");
    let content: Option<Vec<ToolContent>> = field(data, "content");
    let kind = event.kind.as_str();
    update_assistant(source, assistant_id, session_id, |a| {
        for item in &mut a.content {
            let AssistantContent::Tool(tool) = item else {
                continue;
            };
            if tool.id != call_id {
                continue;
            }
            match kind {
                "session.tool.input.delta" => {
                    if let ToolState::Streaming { input } = &mut tool.state {
                        input.push_str(&string(data, "delta"));
                    }
                }
                "session.tool.input.ended" => {
                    if let ToolState::Streaming { input } = &mut tool.state {
                        *input = string(data, "text");
                    }
                }
                "session.tool.called" => {
                    tool.executed = Some(executed);
                    tool.provider_state = field(data, "state");
                    tool.state = ToolState::Running {
                        input: field(data, "input").unwrap_or_default(),
                        metadata: JsonObject::new(),
                    };
                    tool.time.ran = Some(created);
                }
                "session.tool.progress" => {
                    if let ToolState::Running { metadata: m, .. } = &mut tool.state {
                        *m = metadata.clone().unwrap_or_default();
                    }
                }
                "session.tool.success" => {
                    let ToolState::Running { input, .. } = &tool.state else {
                        continue;
                    };
                    let input = input.clone();
                    tool.executed = Some(executed || tool.executed == Some(true));
                    tool.provider_result_state = field(data, "resultState");
                    tool.state = ToolState::Completed {
                        input,
                        content: content.clone().unwrap_or_default(),
                        metadata: metadata.clone(),
                    };
                    tool.time.completed = Some(created);
                }
                "session.tool.failed" => {
                    let (input, prior) = match &tool.state {
                        ToolState::Streaming { .. } => (JsonObject::new(), JsonObject::new()),
                        ToolState::Running { input, metadata } => (input.clone(), metadata.clone()),
                        _ => continue,
                    };
                    tool.executed = Some(executed || tool.executed == Some(true));
                    tool.provider_result_state = field(data, "resultState");
                    tool.state = ToolState::Error {
                        input,
                        error: field(data, "error").unwrap_or(StructuredError {
                            kind: "unknown".into(),
                            message: String::new(),
                        }),
                        content: content.clone(),
                        metadata: Some(metadata.clone().unwrap_or(prior)),
                    };
                    tool.time.completed = Some(created);
                }
                _ => {}
            }
        }
    })
}

fn ordinal(data: &Value) -> usize {
    data.get("ordinal").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn step_snapshot(data: &Value) -> Option<(Option<String>, Option<Vec<String>>)> {
    let end: Option<String> = field(data, "snapshot");
    let files: Option<Vec<String>> = field(data, "files");
    if end.is_none() && files.is_none() {
        return None;
    }
    Some((end, files))
}

fn insert_ordinal(
    content: &mut Vec<AssistantContent>,
    kind: &str,
    ordinal: usize,
    item: AssistantContent,
) {
    if content
        .iter()
        .filter(|c| c.kind() == kind)
        .nth(ordinal)
        .is_some()
    {
        return;
    }
    content.push(item);
}

pub(super) fn update_assistant(
    source: &[SessionMessage],
    id: &str,
    session_id: &str,
    mut apply: impl FnMut(&mut Assistant),
) -> Reduction {
    let mut messages = source.to_vec();
    let mut touched = vec![];
    for m in &mut messages {
        if let SessionMessage::Assistant(a) = m
            && a.id == id
        {
            apply(a);
            if touched.is_empty() {
                touched.push(id.to_owned());
            }
        }
    }
    Reduction {
        session_id: session_id.to_owned(),
        messages,
        touched,
        missing: None,
    }
}
