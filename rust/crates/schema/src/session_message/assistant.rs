//! Purpose: `session-message.ts` assistant content: tool states, assistant tool/text/reasoning
//! parts and the assistant snapshot.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/prompt.rs
//! |-- schema/src/session_message.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- schema/src/session_message.rs
//! Invariants: tag fields keep their baseline struct position; optional `result` keeps `null`.
//! Contract tests: schema/src/session_message.rs (unit tests), schema/tests/schema_session.rs

use super::{
    CompletedTag, CreatedCompleted, ErrorTag, PendingTag, ReasoningTag, RunningTag, TextTag,
    ToolTag, UnknownError, present, tagged_union,
};
use crate::prompt::{FileAttachment, ProviderMetadata, ToolContent};
use crate::values::{DateTimeUtc, JsonObject, RelativePath, optional};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `Session.Message.ToolState.Pending`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStatePending {
    pub status: PendingTag,
    pub input: String,
}

/// `Session.Message.ToolState.Running`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStateRunning {
    pub status: RunningTag,
    pub input: JsonObject,
    pub structured: JsonObject,
    pub content: Vec<ToolContent>,
}

/// `Session.Message.ToolState.Completed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStateCompleted {
    pub status: CompletedTag,
    pub input: JsonObject,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub attachments: Option<Vec<FileAttachment>>,
    pub content: Vec<ToolContent>,
    #[serde(
        rename = "outputPaths",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub output_paths: Option<Vec<String>>,
    pub structured: JsonObject,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present::deserialize"
    )]
    pub result: Option<Value>,
}

/// `Session.Message.ToolState.Error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStateError {
    pub status: ErrorTag,
    pub input: JsonObject,
    pub content: Vec<ToolContent>,
    pub structured: JsonObject,
    pub error: UnknownError,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present::deserialize"
    )]
    pub result: Option<Value>,
}

tagged_union!(
    /// `ToolState`, tagged by `status`.
    ToolState, tag = "status", expected = "ToolState", {
        Pending(ToolStatePending) = "pending",
        Running(ToolStateRunning) = "running",
        Completed(ToolStateCompleted) = "completed",
        Error(ToolStateError) = "error",
    }
);

/// `Session.Message.Assistant.Tool.provider`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantToolProvider {
    pub executed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<ProviderMetadata>,
    #[serde(
        rename = "resultMetadata",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub result_metadata: Option<ProviderMetadata>,
}

/// `Session.Message.Assistant.Tool.time`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantToolTime {
    pub created: DateTimeUtc,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub ran: Option<DateTimeUtc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub completed: Option<DateTimeUtc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub pruned: Option<DateTimeUtc>,
}

/// `Session.Message.Assistant.Tool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantTool {
    pub r#type: ToolTag,
    pub id: String,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub provider: Option<AssistantToolProvider>,
    pub state: ToolState,
    pub time: AssistantToolTime,
}

/// `Session.Message.Assistant.Text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantText {
    pub r#type: TextTag,
    pub id: String,
    pub text: String,
}

/// `Session.Message.Assistant.Reasoning`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantReasoning {
    pub r#type: ReasoningTag,
    pub id: String,
    pub text: String,
    #[serde(
        rename = "providerMetadata",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub provider_metadata: Option<ProviderMetadata>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub time: Option<CreatedCompleted>,
}

tagged_union!(
    /// `AssistantContent`, tagged by `type`.
    AssistantContent, tag = "type", expected = "AssistantContent", {
        Text(AssistantText) = "text",
        Reasoning(AssistantReasoning) = "reasoning",
        Tool(AssistantTool) = "tool",
    }
);

/// `Session.Message.Assistant.snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantSnapshot {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub start: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub end: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub files: Option<Vec<RelativePath>>,
}
