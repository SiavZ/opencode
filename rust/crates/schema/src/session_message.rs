//! Purpose: `session-message.ts` V2 session messages (agent/model switches, user, synthetic,
//! system, shell, assistant, compaction), assistant content parts, tool states and errors,
//! plus the shared `tagged_union!` macro and `Tokens` value used by sibling session modules.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session_message/assistant.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde_json::Value
//! |-- serde_json::Value::deserialize
//! |-- serde_json::from_str
//! |-- serde_json::to_string
//! `-- tagged_union
//! Used by:
//! rust/crates/
//! |-- runtime/src/execution_support.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_aux_event.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_event/stream.rs
//! |-- schema/src/session_message/assistant.rs
//! |-- schema/tests/schema_session.rs
//! |-- session-api/src/message.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/projection/adapter.rs
//! |-- session-runtime/src/projection/updater.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/runner/publish/tools.rs
//! |-- session-runtime/src/runner/turn.rs
//! `-- session-runtime/tests/compaction.rs
//! Invariants: the discriminator stays at its baseline struct position (it is last for `user`,
//! first for `compaction`), spread-overridden keys (`time`) keep the base position, optional
//! `Schema.Unknown` (`result`) keeps an explicit `null`, other optional keys reject `null`.
//! Contract tests: schema/src/session_message.rs (unit tests), schema/tests/schema_session.rs

mod assistant;

pub use assistant::*;

use crate::ids::{MessageID, SessionID};
use crate::prompt::{AgentAttachment, FileAttachment};
use crate::provider::{CacheCost, ModelRef};
use crate::values::{DateTimeUtc, Finite, JsonObject, optional};
use serde::{Deserialize, Serialize};

/// Declares an enum over struct variants that each carry their own literal discriminator
/// field. Encoding delegates to the variant (so the tag keeps its baseline key position),
/// decoding dispatches on the tag value like `Schema.toTaggedUnion`.
macro_rules! tagged_union {
    ($(#[$meta:meta])* $name:ident, tag = $tag:literal, expected = $expected:literal,
     { $($variant:ident($ty:ty) = $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        // Variants mirror baseline payloads by value; boxing would only change ergonomics.
        #[allow(clippy::large_enum_variant)]
        #[derive(Debug, Clone, PartialEq)]
        pub enum $name {
            $($variant($ty)),+
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    $(Self::$variant(value) => value.serialize(serializer)),+
                }
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use ::serde::de::Error;
                let value = ::serde_json::Value::deserialize(deserializer)?;
                let tag = value.get($tag).and_then(::serde_json::Value::as_str).map(str::to_owned);
                match tag.as_deref() {
                    $(Some($value) => <$ty>::deserialize(value)
                        .map(Self::$variant)
                        .map_err(D::Error::custom),)+
                    _ => Err(D::Error::custom(format!("Expected {}, got {}", $expected, value))),
                }
            }
        }
    };
}
pub(crate) use tagged_union;

/// Deserializer for optional `Schema.Unknown` fields: absent -> `None`, any present value
/// (including `null`) -> `Some`. Pair with `default, skip_serializing_if = "Option::is_none"`.
pub mod present {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<serde_json::Value>, D::Error> {
        serde_json::Value::deserialize(deserializer).map(Some)
    }
}

crate::literal!(UnknownTag = "unknown");
crate::literal!(AgentSwitchedTag = "agent-switched");
crate::literal!(ModelSwitchedTag = "model-switched");
crate::literal!(UserTag = "user");
crate::literal!(SyntheticTag = "synthetic");
crate::literal!(SystemTag = "system");
crate::literal!(ShellTag = "shell");
crate::literal!(AssistantTag = "assistant");
crate::literal!(CompactionTag = "compaction");
crate::literal!(PendingTag = "pending");
crate::literal!(RunningTag = "running");
crate::literal!(CompletedTag = "completed");
crate::literal!(ErrorTag = "error");
crate::literal!(ToolTag = "tool");
crate::literal!(TextTag = "text");
crate::literal!(ReasoningTag = "reasoning");

/// `Session.Error.Unknown`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownError {
    pub r#type: UnknownTag,
    pub message: String,
}

impl UnknownError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            r#type: UnknownTag,
            message: message.into(),
        }
    }
}

/// `{ input, output, reasoning, cache: { read, write } }` token usage.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub input: Finite,
    pub output: Finite,
    pub reasoning: Finite,
    pub cache: CacheCost,
}

/// Base `time` of most messages: `{ created }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Created {
    pub created: DateTimeUtc,
}

/// `{ created, completed? }` time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedCompleted {
    pub created: DateTimeUtc,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub completed: Option<DateTimeUtc>,
}

/// `Session.Message.AgentSwitched`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSwitched {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
    pub r#type: AgentSwitchedTag,
    pub agent: String,
}

/// `Session.Message.ModelSwitched`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSwitched {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
    pub r#type: ModelSwitchedTag,
    pub model: ModelRef,
}

/// `Session.Message.User` (prompt fields spread before the tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub files: Option<Vec<FileAttachment>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub agents: Option<Vec<AgentAttachment>>,
    pub r#type: UserTag,
}

/// `Session.Message.Synthetic`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Synthetic {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub text: String,
    pub r#type: SyntheticTag,
}

/// `Session.Message.System`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct System {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
    pub r#type: SystemTag,
    pub text: String,
}

/// `Session.Message.Shell` (`time` keeps the base position).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shell {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: CreatedCompleted,
    pub r#type: ShellTag,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub command: String,
    pub output: String,
}

/// `Session.Message.Assistant` (`time` keeps the base position).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assistant {
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: CreatedCompleted,
    pub r#type: AssistantTag,
    pub agent: String,
    pub model: ModelRef,
    pub content: Vec<AssistantContent>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub snapshot: Option<AssistantSnapshot>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub finish: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub cost: Option<Finite>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub tokens: Option<Tokens>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub error: Option<UnknownError>,
}

/// `Compaction.reason` and compaction event `reason`: `"auto" | "manual"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    Auto,
    Manual,
}

/// `Session.Message.Compaction` (tag and fields before the base spread).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    pub r#type: CompactionTag,
    pub reason: CompactionReason,
    pub summary: String,
    pub recent: String,
    pub id: MessageID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    pub time: Created,
}

tagged_union!(
    /// `Session.Message`, tagged by `type`.
    Message, tag = "type", expected = "Session.Message", {
        AgentSwitched(AgentSwitched) = "agent-switched",
        ModelSwitched(ModelSwitched) = "model-switched",
        User(User) = "user",
        Synthetic(Synthetic) = "synthetic",
        System(System) = "system",
        Shell(Shell) = "shell",
        Assistant(Assistant) = "assistant",
        Compaction(Compaction) = "compaction",
    }
);

impl Message {
    /// The message ID shared by every variant.
    pub fn id(&self) -> &MessageID {
        match self {
            Message::AgentSwitched(m) => &m.id,
            Message::ModelSwitched(m) => &m.id,
            Message::User(m) => &m.id,
            Message::Synthetic(m) => &m.id,
            Message::System(m) => &m.id,
            Message::Shell(m) => &m.id,
            Message::Assistant(m) => &m.id,
            Message::Compaction(m) => &m.id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt<T: Serialize + for<'de> Deserialize<'de>>(input: &str) -> String {
        serde_json::to_string(&serde_json::from_str::<T>(input).unwrap()).unwrap()
    }

    #[test]
    fn tag_keeps_struct_position() {
        // fixture: captured from bun at baseline (packages/schema 907b3bc51).
        assert_eq!(
            rt::<Message>(
                r#"{"type":"user","id":"msg_1","time":{"created":5},"text":"t","files":[{"uri":"u","mime":"m"}]}"#
            ),
            r#"{"id":"msg_1","time":{"created":5},"text":"t","files":[{"uri":"u","mime":"m"}],"type":"user"}"#
        );
        assert_eq!(
            rt::<Message>(
                r#"{"id":"msg_1","time":{"created":5},"type":"compaction","reason":"auto","summary":"s","recent":"r"}"#
            ),
            r#"{"type":"compaction","reason":"auto","summary":"s","recent":"r","id":"msg_1","time":{"created":5}}"#
        );
    }

    #[test]
    fn result_keeps_null_and_other_optionals_reject_it() {
        // fixture: captured from bun at baseline.
        assert_eq!(
            rt::<ToolState>(
                r#"{"status":"completed","input":{},"content":[],"structured":{},"result":null}"#
            ),
            r#"{"status":"completed","input":{},"content":[],"structured":{},"result":null}"#
        );
        assert_eq!(
            rt::<ToolState>(r#"{"status":"completed","input":{},"content":[],"structured":{}}"#),
            r#"{"status":"completed","input":{},"content":[],"structured":{}}"#
        );
        let null_finish = r#"{"type":"assistant","id":"msg_1","time":{"created":5},"agent":"a","model":{"id":"m","providerID":"p"},"content":[],"finish":null}"#;
        assert!(serde_json::from_str::<Message>(null_finish).is_err());
    }

    #[test]
    fn unknown_or_missing_tag_fails() {
        let error =
            serde_json::from_str::<Message>(r#"{"type":"nope","id":"msg_1","time":{"created":5}}"#)
                .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Expected Session.Message, got")
        );
        assert!(serde_json::from_str::<Message>(r#"{"id":"msg_1","time":{"created":5}}"#).is_err());
        assert!(
            serde_json::from_str::<Message>(
                r#"{"type":"system","id":"bad","time":{"created":5},"text":"x"}"#
            )
            .is_err()
        );
    }
}
