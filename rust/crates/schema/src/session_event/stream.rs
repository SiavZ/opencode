//! Purpose: `session-event.ts` step, text, reasoning, tool, retry, compaction and revert event
//! payloads (the assistant streaming half of the session event surface).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/revert.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_message.rs
//! `-- schema/src/values.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! `-- schema/src/session_event.rs
//! Invariants: field order follows the baseline spreads (`Base`, then `ToolBase` =
//! `assistantMessageID, callID`); delta events carry no durable spec.
//! Contract tests: schema/src/session_event/stream.rs (unit tests), schema/tests/schema_session.rs

use super::{V1, V2, session_event};
use crate::ids::MessageID;
use crate::prompt::{ProviderMetadata, ToolContent};
use crate::provider::ModelRef;
use crate::revert;
use crate::session_message::{CompactionReason, Tokens, UnknownError, present};
use crate::values::{Finite, JsonObject, RelativePath, optional};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

session_event!(StepStarted = "session.next.step.started", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    pub agent: String,
    pub model: ModelRef,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub snapshot: Option<String>,
});

session_event!(StepEnded = "session.next.step.ended", V2, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    pub finish: String,
    pub cost: Finite,
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub files: Option<Vec<RelativePath>>,
});

session_event!(StepFailed = "session.next.step.failed", V2, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    pub error: UnknownError,
});

session_event!(TextStarted = "session.next.text.started", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "textID")]
    pub text_id: String,
});

session_event!(
    /// Live-only stream fragment; `TextEnded` is the replayable boundary.
    TextDelta = "session.next.text.delta", None, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "textID")]
    pub text_id: String,
    pub delta: String,
});

session_event!(TextEnded = "session.next.text.ended", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "textID")]
    pub text_id: String,
    pub text: String,
});

session_event!(ReasoningStarted = "session.next.reasoning.started", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "reasoningID")]
    pub reasoning_id: String,
    #[serde(rename = "providerMetadata", default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub provider_metadata: Option<ProviderMetadata>,
});

session_event!(
    /// Live-only stream fragment; `ReasoningEnded` is the replayable boundary.
    ReasoningDelta = "session.next.reasoning.delta", None, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "reasoningID")]
    pub reasoning_id: String,
    pub delta: String,
});

session_event!(ReasoningEnded = "session.next.reasoning.ended", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "reasoningID")]
    pub reasoning_id: String,
    pub text: String,
    #[serde(rename = "providerMetadata", default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub provider_metadata: Option<ProviderMetadata>,
});

session_event!(ToolInputStarted = "session.next.tool.input.started", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub name: String,
});

session_event!(
    /// Live-only stream fragment; `ToolInputEnded` is the replayable raw-input boundary.
    ToolInputDelta = "session.next.tool.input.delta", None, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub delta: String,
});

session_event!(ToolInputEnded = "session.next.tool.input.ended", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub text: String,
});

/// Tool event `provider`: `{ executed, metadata? }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolProvider {
    pub executed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<ProviderMetadata>,
}

session_event!(ToolCalled = "session.next.tool.called", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub tool: String,
    pub input: JsonObject,
    pub provider: ToolProvider,
});

session_event!(
    /// Replayable bounded running-tool state checkpoint.
    ToolProgress = "session.next.tool.progress", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub structured: JsonObject,
    pub content: Vec<ToolContent>,
});

session_event!(ToolSuccess = "session.next.tool.success", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub structured: JsonObject,
    pub content: Vec<ToolContent>,
    #[serde(rename = "outputPaths", default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub output_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present::deserialize")]
    pub result: Option<Value>,
    pub provider: ToolProvider,
});

session_event!(ToolFailed = "session.next.tool.failed", V1, {
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub error: UnknownError,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present::deserialize")]
    pub result: Option<Value>,
    pub provider: ToolProvider,
});

/// `session.next.retry_error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryError {
    pub message: String,
    #[serde(
        rename = "statusCode",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub status_code: Option<Finite>,
    #[serde(rename = "isRetryable")]
    pub is_retryable: bool,
    #[serde(
        rename = "responseHeaders",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub response_headers: Option<IndexMap<String, String>>,
    #[serde(
        rename = "responseBody",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub response_body: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<IndexMap<String, String>>,
}

session_event!(Retried = "session.next.retried", V1, {
    pub attempt: Finite,
    pub error: RetryError,
});

session_event!(CompactionStarted = "session.next.compaction.started", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub reason: CompactionReason,
});

session_event!(
    /// Live-only compaction summary fragment.
    CompactionDelta = "session.next.compaction.delta", None, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub text: String,
});

session_event!(CompactionEnded = "session.next.compaction.ended", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub reason: CompactionReason,
    pub text: String,
    pub recent: String,
});

session_event!(RevertStaged = "session.next.revert.staged", V1, {
    pub revert: revert::State,
});

session_event!(RevertCleared = "session.next.revert.cleared", V1, {});

session_event!(RevertCommitted = "session.next.revert.committed", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    fn rt<T: Serialize + for<'de> Deserialize<'de>>(input: &str) -> String {
        serde_json::to_string(&serde_json::from_str::<T>(input).unwrap()).unwrap()
    }

    #[test]
    fn step_ended_matches_baseline() {
        // fixture: captured from bun at baseline (packages/schema 907b3bc51).
        let input = r#"{"type":"session.next.step.ended","id":"evt_1","data":{"files":["a"],"tokens":{"cache":{"write":1,"read":2},"reasoning":0,"output":1,"input":2},"cost":0.5,"finish":"stop","assistantMessageID":"msg_1","sessionID":"ses_1","timestamp":10}}"#;
        assert_eq!(
            rt::<Event<StepEnded>>(input),
            r#"{"id":"evt_1","type":"session.next.step.ended","data":{"timestamp":10,"sessionID":"ses_1","assistantMessageID":"msg_1","finish":"stop","cost":0.5,"tokens":{"input":2,"output":1,"reasoning":0,"cache":{"read":2,"write":1}},"files":["a"]}}"#
        );
    }

    #[test]
    fn tool_called_matches_baseline() {
        // fixture: captured from bun at baseline.
        let input = r#"{"id":"evt_1","type":"session.next.tool.called","data":{"provider":{"executed":true},"input":{"b":1,"a":2},"tool":"bash","callID":"c","assistantMessageID":"msg_1","sessionID":"ses_1","timestamp":3}}"#;
        assert_eq!(
            rt::<Event<ToolCalled>>(input),
            r#"{"id":"evt_1","type":"session.next.tool.called","data":{"timestamp":3,"sessionID":"ses_1","assistantMessageID":"msg_1","callID":"c","tool":"bash","input":{"b":1,"a":2},"provider":{"executed":true}}}"#
        );
    }

    #[test]
    fn invalid_payloads_fail() {
        let bad_reason =
            r#"{"timestamp":1,"sessionID":"ses_1","messageID":"msg_1","reason":"never"}"#;
        assert!(serde_json::from_str::<CompactionStarted>(bad_reason).is_err());
        let null_status = r#"{"message":"m","isRetryable":true,"statusCode":null}"#;
        assert!(serde_json::from_str::<RetryError>(null_status).is_err());
        let cleared: RevertCleared =
            serde_json::from_str(r#"{"sessionID":"ses_1","timestamp":4}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&cleared).unwrap(),
            r#"{"timestamp":4,"sessionID":"ses_1"}"#
        );
    }
}
