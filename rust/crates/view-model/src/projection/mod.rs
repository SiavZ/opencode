//! Purpose: project canonical source messages into render messages and parts with stable IDs.
//! Port of `packages/app/src/utils/session-message.ts:15-366` (`normalizeSessionMessages`,
//! `compareMessages`, `sessionMessagePartID`, tool input/metadata normalization).
//! Scope: pure transform, renderer-agnostic (browser timeline and terminal both consume it).
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/tool.rs
//! `-- view-model/src/source.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! |-- view-model/src/timeline/mod.rs
//! `-- view-model/tests/projection.rs
//! Invariants: part IDs are `${msg}:text:${n}`, `${msg}:reasoning:${n}`, `${msg}:file:${i}`,
//! `${msg}:agent:${i}`, `${compaction}:compaction`, tool call id, `${tool}:file:${i}`; shell
//! messages yield a user `${id}` and assistant `${id}:assistant` with tool part `${id}:tool`.
//! Assistants without a preceding user root are dropped (no invented parent).
//! Contract tests: view-model/tests/projection.rs

mod tool;

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::source::{
    Assistant, AssistantContent, JsonObject, ModelRef, SessionMessage, SpanTime, TokenUsage,
};

/// Error class shown by the timeline; `aborted` drives the interrupted divider.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderError {
    pub aborted: bool,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    User,
    Assistant {
        parent_id: String,
        error: Option<RenderError>,
        finish: Option<String>,
        cost: f64,
        tokens: Option<TokenUsage>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderMessage {
    pub id: String,
    pub time: SpanTime,
    pub agent: String,
    pub model: ModelRef,
    pub role: Role,
    /// Legacy per-turn summary diffs (only legacy producers set them).
    pub summary_diffs: Vec<SummaryDiff>,
}

impl RenderMessage {
    pub fn is_user(&self) -> bool {
        matches!(self.role, Role::User)
    }
    pub fn parent_id(&self) -> Option<&str> {
        match &self.role {
            Role::Assistant { parent_id, .. } => Some(parent_id),
            Role::User => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryDiff {
    pub file: String,
    pub additions: i64,
    pub deletions: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolStatus {
    Pending {
        raw: String,
    },
    Running,
    Completed {
        output: String,
        title: String,
        attachments: Vec<RenderPart>,
    },
    Error {
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PartKind {
    Text {
        text: String,
        synthetic: bool,
    },
    Reasoning {
        text: String,
        start: i64,
        end: Option<i64>,
    },
    File {
        mime: String,
        filename: Option<String>,
        url: String,
        mention_path: Option<String>,
    },
    Agent {
        name: String,
    },
    Tool {
        tool: String,
        call_id: String,
        input: JsonObject,
        metadata: JsonObject,
        status: ToolStatus,
        start: i64,
        end: Option<i64>,
    },
    Compaction {
        auto: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderPart {
    pub id: String,
    pub message_id: String,
    pub kind: PartKind,
}

impl RenderPart {
    pub fn type_name(&self) -> &'static str {
        match self.kind {
            PartKind::Text { .. } => "text",
            PartKind::Reasoning { .. } => "reasoning",
            PartKind::File { .. } => "file",
            PartKind::Agent { .. } => "agent",
            PartKind::Tool { .. } => "tool",
            PartKind::Compaction { .. } => "compaction",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projection {
    pub messages: Vec<RenderMessage>,
    pub parts: HashMap<String, Vec<RenderPart>>,
}

impl Projection {
    pub fn message(&self, id: &str) -> Option<&RenderMessage> {
        self.messages.iter().find(|m| m.id == id)
    }
    pub fn parts_of(&self, id: &str) -> &[RenderPart] {
        self.parts.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub fn part_id(message_id: &str, kind: &str, ordinal: usize) -> String {
    format!("{message_id}:{kind}:{ordinal}")
}

/// Ordering key `time.created + id` (string concatenation, as the baseline compares).
pub fn message_key(created: i64, id: &str) -> String {
    format!("{created}{id}")
}

pub fn project(source: &[SessionMessage]) -> Projection {
    let mut out = Projection::default();
    let mut agent = String::new();
    let mut model = ModelRef {
        id: String::new(),
        provider_id: String::new(),
        variant: None,
    };
    let mut parent: Option<String> = None;
    let user = |id: &str, created: i64, agent: &str, model: &ModelRef| RenderMessage {
        id: id.to_owned(),
        time: SpanTime {
            created,
            completed: None,
        },
        agent: agent.to_owned(),
        model: model.clone(),
        role: Role::User,
        summary_diffs: vec![],
    };
    for message in source {
        match message {
            SessionMessage::AgentSwitched { agent: a, .. } => agent = a.clone(),
            SessionMessage::ModelSwitched { model: m, .. } => model = m.clone(),
            SessionMessage::User {
                id,
                time,
                text,
                files,
                agents,
                ..
            } => {
                parent = Some(id.clone());
                out.messages.push(user(id, time.created, &agent, &model));
                let mut parts = vec![text_part(id, 0, text, false)];
                for (i, f) in files.iter().flatten().enumerate() {
                    let url = match f.source.get("type").and_then(Value::as_str) {
                        Some("uri") => f
                            .source
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        _ => format!("data:{};base64,{}", f.mime, f.data),
                    };
                    let mention_path = f.mention.as_ref().map(|m| match m.text.strip_prefix('@') {
                        Some(path) => path.to_owned(),
                        None => f.name.clone().unwrap_or_else(|| m.text.clone()),
                    });
                    parts.push(RenderPart {
                        id: format!("{id}:file:{i}"),
                        message_id: id.clone(),
                        kind: PartKind::File {
                            mime: f.mime.clone(),
                            filename: f.name.clone(),
                            url,
                            mention_path,
                        },
                    });
                }
                for (i, a) in agents.iter().flatten().enumerate() {
                    parts.push(RenderPart {
                        id: format!("{id}:agent:{i}"),
                        message_id: id.clone(),
                        kind: PartKind::Agent {
                            name: a.name.clone(),
                        },
                    });
                }
                out.parts.insert(id.clone(), parts);
            }
            SessionMessage::Synthetic {
                id,
                time,
                description: Some(d),
                ..
            } if !d.trim().is_empty() => {
                parent = Some(id.clone());
                out.messages.push(user(id, time.created, &agent, &model));
                out.parts
                    .insert(id.clone(), vec![text_part(id, 0, d, true)]);
            }
            SessionMessage::Shell {
                id,
                time,
                shell_id,
                command,
                status,
                exit,
                output,
                ..
            } => {
                out.messages.push(user(id, time.created, &agent, &model));
                let assistant_id = format!("{id}:assistant");
                out.messages.push(RenderMessage {
                    id: assistant_id.clone(),
                    time: time.clone(),
                    agent: agent.clone(),
                    model: model.clone(),
                    role: Role::Assistant {
                        parent_id: id.clone(),
                        error: None,
                        finish: None,
                        cost: 0.0,
                        tokens: None,
                    },
                    summary_diffs: vec![],
                });
                out.parts
                    .insert(id.clone(), vec![text_part(id, 0, command, false)]);
                let mut input = JsonObject::new();
                input.insert("command".into(), json!(command));
                let (status, end, metadata) = if status == "running" {
                    (ToolStatus::Running, None, JsonObject::new())
                } else {
                    let mut meta = JsonObject::new();
                    meta.insert("status".into(), json!(status));
                    if let Some(exit) = exit {
                        meta.insert("exit".into(), exit.clone());
                    }
                    if let Some(o) = output {
                        meta.insert("truncated".into(), json!(o.truncated));
                    }
                    let output = output
                        .as_ref()
                        .map(|o| o.output.clone())
                        .unwrap_or_default();
                    let done = ToolStatus::Completed {
                        output,
                        title: "Shell".into(),
                        attachments: vec![],
                    };
                    (done, Some(time.completed.unwrap_or(time.created)), meta)
                };
                out.parts.insert(
                    assistant_id.clone(),
                    vec![RenderPart {
                        id: format!("{id}:tool"),
                        message_id: assistant_id,
                        kind: PartKind::Tool {
                            tool: "bash".into(),
                            call_id: shell_id.clone(),
                            input,
                            metadata,
                            status,
                            start: time.created,
                            end,
                        },
                    }],
                );
                parent = None;
            }
            SessionMessage::Assistant(a) => {
                agent = a.agent.clone();
                model = a.model.clone();
                let Some(parent_id) = parent.clone() else {
                    continue;
                };
                if let Some(p) = out.messages.iter_mut().rev().find(|m| m.id == parent_id)
                    && p.is_user()
                {
                    p.agent = a.agent.clone();
                    p.model = a.model.clone();
                }
                out.messages.push(assistant_message(&parent_id, a));
                out.parts.insert(a.id.clone(), assistant_parts(a));
            }
            SessionMessage::Compaction { id, reason, .. } => {
                let Some(parent_id) = parent.clone() else {
                    continue;
                };
                out.parts
                    .entry(parent_id.clone())
                    .or_default()
                    .push(RenderPart {
                        id: format!("{id}:compaction"),
                        message_id: parent_id,
                        kind: PartKind::Compaction {
                            auto: reason == "auto",
                        },
                    });
            }
            _ => {}
        }
    }
    out
}

fn text_part(message_id: &str, ordinal: usize, text: &str, synthetic: bool) -> RenderPart {
    RenderPart {
        id: part_id(message_id, "text", ordinal),
        message_id: message_id.to_owned(),
        kind: PartKind::Text {
            text: text.to_owned(),
            synthetic,
        },
    }
}

fn assistant_message(parent_id: &str, a: &Assistant) -> RenderMessage {
    let error = a.error.as_ref().map(|e| {
        let kind = e.kind.to_lowercase();
        RenderError {
            aborted: kind.contains("abort") || kind.contains("interrupt"),
            message: e.message.clone(),
        }
    });
    RenderMessage {
        id: a.id.clone(),
        time: a.time.clone(),
        agent: a.agent.clone(),
        model: a.model.clone(),
        role: Role::Assistant {
            parent_id: parent_id.to_owned(),
            error,
            finish: a.finish.clone(),
            cost: a.cost.unwrap_or(0.0),
            tokens: a.tokens.clone(),
        },
        summary_diffs: vec![],
    }
}

fn assistant_parts(a: &Assistant) -> Vec<RenderPart> {
    let (mut text_n, mut reasoning_n) = (0, 0);
    let mut parts = vec![];
    for content in &a.content {
        match content {
            AssistantContent::Text { text } => {
                let part = text_part(&a.id, text_n, text, false);
                text_n += 1;
                if !text.trim().is_empty() {
                    parts.push(part);
                }
            }
            AssistantContent::Reasoning { text, time, .. } => {
                let id = part_id(&a.id, "reasoning", reasoning_n);
                reasoning_n += 1;
                if text.trim().is_empty() {
                    continue;
                }
                parts.push(RenderPart {
                    id,
                    message_id: a.id.clone(),
                    kind: PartKind::Reasoning {
                        text: text.clone(),
                        start: time.as_ref().map(|t| t.created).unwrap_or(a.time.created),
                        end: time.as_ref().and_then(|t| t.completed),
                    },
                });
            }
            AssistantContent::Tool(t) => parts.push(tool::tool_part(&a.id, t)),
        }
    }
    parts
}
