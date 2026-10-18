//! Purpose: canonical V2 session source values (`SessionMessageInfo` in the baseline client)
//! and the event envelope the reducer consumes. Mirrors
//! `packages/app/node_modules/@opencode-ai/client/dist/promise/generated/types.d.ts`.
//! Scope: serde data types only. Phase 06 `opencode-protocol` will own the wire DTOs, at
//! which point these become re-exports or thin conversions.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! |-- view-model/src/reducer/assistant.rs
//! |-- view-model/src/reducer/mod.rs
//! `-- view-model/tests/reducer.rs
//! Invariants: optional keys are omitted when absent (never serialized as null), times are
//! integer milliseconds, tagged unions use the baseline `type`/`status` discriminators.
//! Contract tests: view-model/tests/reducer.rs

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuredError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenCache {
    pub read: f64,
    pub write: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: f64,
    pub output: f64,
    pub reasoning: f64,
    pub cache: TokenCache,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CreatedTime {
    pub created: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpanTime {
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Retry {
    pub attempt: i64,
    pub at: i64,
    pub error: StructuredError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellOutput {
    pub output: String,
    pub cursor: i64,
    pub size: i64,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text {
        text: String,
    },
    File {
        uri: String,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolState {
    Streaming {
        input: String,
    },
    Running {
        input: JsonObject,
        metadata: JsonObject,
    },
    Completed {
        input: JsonObject,
        content: Vec<ToolContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
    },
    Error {
        input: JsonObject,
        error: StructuredError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ToolContent>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolTime {
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ran: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolPart {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed: Option<bool>,
    #[serde(
        rename = "providerState",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_state: Option<JsonObject>,
    #[serde(
        rename = "providerResultState",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_result_state: Option<JsonObject>,
    pub state: ToolState,
    pub time: ToolTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<SpanTime>,
    },
    Tool(Box<ToolPart>),
}

impl AssistantContent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Reasoning { .. } => "reasoning",
            Self::Tool(_) => "tool",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mention {
    pub start: i64,
    pub end: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub data: String,
    pub mime: String,
    pub source: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention: Option<Mention>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentAttachment {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention: Option<Mention>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assistant {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    pub time: SpanTime,
    pub agent: String,
    pub model: ModelRef,
    pub content: Vec<AssistantContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum CompactionState {
    Running { summary: String, recent: String },
    Completed { summary: String, recent: String },
    Failed { error: StructuredError },
}

/// One canonical session message. Serialized with the baseline `type` discriminator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionMessage {
    #[serde(rename = "agent-switched")]
    AgentSwitched {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        agent: String,
    },
    #[serde(rename = "model-switched")]
    ModelSwitched {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        model: ModelRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<ModelRef>,
    },
    #[serde(rename = "user")]
    User {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files: Option<Vec<FileAttachment>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<AgentAttachment>>,
    },
    #[serde(rename = "synthetic")]
    Synthetic {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    #[serde(rename = "system")]
    System {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        text: String,
    },
    #[serde(rename = "skill")]
    Skill {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        skill: String,
        name: String,
        text: String,
    },
    #[serde(rename = "shell")]
    Shell {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: SpanTime,
        #[serde(rename = "shellID")]
        shell_id: String,
        command: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<ShellOutput>,
    },
    #[serde(rename = "assistant")]
    Assistant(Assistant),
    #[serde(rename = "compaction")]
    Compaction {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        time: CreatedTime,
        reason: String,
        #[serde(flatten)]
        state: CompactionState,
    },
}

impl SessionMessage {
    pub fn id(&self) -> &str {
        match self {
            Self::AgentSwitched { id, .. }
            | Self::ModelSwitched { id, .. }
            | Self::User { id, .. }
            | Self::Synthetic { id, .. }
            | Self::System { id, .. }
            | Self::Skill { id, .. }
            | Self::Shell { id, .. }
            | Self::Compaction { id, .. } => id,
            Self::Assistant(a) => &a.id,
        }
    }

    pub fn created(&self) -> i64 {
        match self {
            Self::AgentSwitched { time, .. }
            | Self::ModelSwitched { time, .. }
            | Self::User { time, .. }
            | Self::Synthetic { time, .. }
            | Self::System { time, .. }
            | Self::Skill { time, .. }
            | Self::Compaction { time, .. } => time.created,
            Self::Shell { time, .. } => time.created,
            Self::Assistant(a) => a.time.created,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::AgentSwitched { .. } => "agent-switched",
            Self::ModelSwitched { .. } => "model-switched",
            Self::User { .. } => "user",
            Self::Synthetic { .. } => "synthetic",
            Self::System { .. } => "system",
            Self::Skill { .. } => "skill",
            Self::Shell { .. } => "shell",
            Self::Assistant(_) => "assistant",
            Self::Compaction { .. } => "compaction",
        }
    }

    pub fn as_assistant(&self) -> Option<&Assistant> {
        match self {
            Self::Assistant(a) => Some(a),
            _ => None,
        }
    }
}

/// Event envelope as delivered by the V2 event stream (`OpenCodeEvent`). `data` stays a raw
/// JSON object so unknown event types pass through and the reducer decodes per type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub id: String,
    pub created: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Value>,
    #[serde(default)]
    pub data: Value,
}

/// Pending (admitted, not yet promoted) prompt input, `SessionPendingMessage`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingInput {
    #[serde(rename = "type")]
    pub kind: String,
    pub delivery: String,
    pub data: PendingData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingData {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}
