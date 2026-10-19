//! Purpose: legacy `SessionV1` message/part values (`WithParts`, `Info`, `Part`) kept as ordered
//! JSON objects so every field round-trips byte-identically, plus the typed `LegacyError` and the
//! prompt input values of packages/opencode/src/session/prompt.ts:1494-1562.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/compaction.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/resource.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/share.rs
//! |-- session-runtime/src/legacy/shell.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/summary.rs
//! |-- session-runtime/src/title.rs
//! |-- session-runtime/tests/compaction_legacy.rs
//! |-- session-runtime/tests/legacy_command.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! |-- session-runtime/tests/legacy_shell.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/title.rs
//! Invariants: objects keep insertion order (serde_json `preserve_order`), matching the key order of
//! the baseline object literals. `LegacyError` keeps busy/not-found/cancelled/not-yet-ported distinct.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A legacy message `info` or part as an ordered JSON object.
pub type Object = Map<String, Value>;

/// `SessionV1.WithParts`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WithParts {
    pub info: Object,
    pub parts: Vec<Object>,
}

impl WithParts {
    pub fn id(&self) -> &str {
        str_field(&self.info, "id").unwrap_or_default()
    }
    pub fn role(&self) -> &str {
        str_field(&self.info, "role").unwrap_or_default()
    }
    pub fn is_user(&self) -> bool {
        self.role() == "user"
    }
    pub fn is_assistant(&self) -> bool {
        self.role() == "assistant"
    }
    /// `parts.some(p => p.type === kind)`.
    pub fn has_part(&self, kind: &str) -> bool {
        self.parts.iter().any(|part| part_type(part) == kind)
    }
}

pub fn str_field<'a>(object: &'a Object, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

pub fn part_type(part: &Object) -> &str {
    str_field(part, "type").unwrap_or_default()
}

/// `{ providerID, modelID, variant? }` as used by legacy prompt input and user messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl ModelRef {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            variant: None,
        }
    }
}

/// `SessionPrompt.PromptInput`. Parts stay JSON (`TextPartInput | FilePartInput | AgentPartInput |
/// SubtaskPartInput`), discriminated by `type`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptInput {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID", default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(rename = "noReply", default, skip_serializing_if = "Option::is_none")]
    pub no_reply: Option<bool>,
    /// Deprecated `tools` map, ordered like the input object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub parts: Vec<Object>,
}

/// `SessionPrompt.ShellInput`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellInput {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID", default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    pub command: String,
}

/// `SessionPrompt.CommandInput`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandInput {
    #[serde(rename = "messageID", default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `provider/model` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub arguments: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<Object>>,
}

/// Legacy `Agent.Info` fields read by the prompt paths.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    /// `subagent | primary | all`.
    pub mode: String,
    #[serde(default)]
    pub hidden: Option<bool>,
    /// `PermissionV1.Ruleset` (`{ permission, pattern, action }`).
    #[serde(default)]
    pub permission: Vec<Object>,
    #[serde(default)]
    pub model: Option<ModelRef>,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub steps: Option<f64>,
}

/// Typed failures of the legacy Session paths. `Clone` so one run result can be joined by
/// several callers (`Runner.ensureRunning`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LegacyError {
    /// `Session.NotFoundError`.
    #[error("Session not found: {0}")]
    NotFound(String),
    /// `Session.BusyError` (`SessionBusyError`).
    #[error("Session {0} is busy")]
    Busy(String),
    /// `NamedError.Unknown` thrown after publishing `session.error`.
    #[error("{0}")]
    Unknown(String),
    /// `Provider.ModelNotFoundError`.
    #[error("Model not found: {provider_id}/{model_id}")]
    ModelNotFound {
        provider_id: String,
        model_id: String,
        suggestions: Vec<String>,
    },
    /// The run or shell was interrupted (`RunnerCancelled`) and no interrupt fallback applied.
    #[error("operation cancelled")]
    Cancelled,
    /// A capability whose upstream adapter is not ported yet. Never a silent success.
    #[error("not yet ported: {0}")]
    NotYetPorted(&'static str),
    /// Storage or event infrastructure failure (`Effect.orDie` in the baseline).
    #[error("storage failure: {message}")]
    Storage { message: String, retryable: bool },
    /// Process failure that is not an interruption (`Effect.failCause(exit.cause)`).
    #[error("{0}")]
    Process(String),
}

impl LegacyError {
    pub fn storage(error: impl std::fmt::Display) -> Self {
        Self::Storage {
            message: error.to_string(),
            retryable: false,
        }
    }
}

impl From<opencode_session_api::SessionError> for LegacyError {
    fn from(error: opencode_session_api::SessionError) -> Self {
        use opencode_session_api::SessionError as E;
        match error {
            E::NotFound { session_id } => Self::NotFound(session_id.as_str().to_owned()),
            E::Cancelled => Self::Cancelled,
            E::Storage { message, retryable } => Self::Storage { message, retryable },
            other => Self::Storage {
                message: other.to_string(),
                retryable: false,
            },
        }
    }
}

impl From<opencode_storage_api::StorageError> for LegacyError {
    fn from(error: opencode_storage_api::StorageError) -> Self {
        Self::Storage {
            retryable: error.is_retryable(),
            message: error.to_string(),
        }
    }
}

impl From<opencode_event_api::EventError> for LegacyError {
    fn from(error: opencode_event_api::EventError) -> Self {
        Self::Storage {
            retryable: error.is_retryable(),
            message: error.to_string(),
        }
    }
}

/// `NamedError.Unknown({ message }).toObject()`.
pub fn unknown_error(message: &str) -> Value {
    serde_json::json!({ "name": "UnknownError", "data": { "message": message } })
}

pub fn now_millis() -> i64 {
    opencode_schema::values::DateTimeUtc::now().millis()
}

/// `MessageID.ascending()` (legacy `msg` prefix).
pub fn message_id() -> String {
    opencode_schema::ids::v1::MessageID::ascending(None).into_inner()
}

/// `PartID.ascending()` (legacy `prt` prefix).
pub fn part_id() -> String {
    opencode_schema::ids::v1::PartID::ascending(None).into_inner()
}
