//! Purpose: `session-event.ts` durable V2 session events: definitions, payload structs, the
//! `SessionDurableEvent` and all-events tagged unions, and their inventories.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/location.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session_event/stream.rs
//! |-- schema/src/session_input.rs
//! |-- schema/src/session_message.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! `-- schema/src/session_event/stream.rs
//! Invariants: every payload starts with `timestamp, sessionID` (the baseline `Base` spread);
//! durable events use aggregate `sessionID`, version 1 except step ended/failed (version 2);
//! delta events are live-only (absent from DURABLE_DEFINITIONS and `DurableEvent`).
//! Contract tests: schema/src/session_event.rs (unit tests), schema/tests/schema_session.rs

mod stream;

pub use stream::*;

use crate::event::{Definition, DurableSpec, Event};
use crate::ids::MessageID;
use crate::location;
use crate::prompt::Prompt;
use crate::provider::ModelRef;
use crate::session_input::Delivery;
use crate::session_message::tagged_union;
use crate::values::{NonNegativeInt, RelativePath, optional};
use serde::{Deserialize, Serialize};

pub use crate::prompt::FileAttachment;
pub use crate::session_message::UnknownError;

pub(crate) const V1: Option<DurableSpec> = Some(DurableSpec {
    version: 1,
    aggregate: "sessionID",
});
pub(crate) const V2: Option<DurableSpec> = Some(DurableSpec {
    version: 2,
    aggregate: "sessionID",
});

/// Declares an event payload struct and its `EventData` definition.
macro_rules! session_event {
    ($(#[$meta:meta])* $name:ident = $type:literal, $durable:expr, { $($body:tt)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
        pub struct $name {
            pub timestamp: $crate::values::DateTimeUtc,
            #[serde(rename = "sessionID")]
            pub session_id: $crate::ids::SessionID,
            $($body)*
        }

        impl $crate::event::EventData for $name {
            const DEFINITION: $crate::event::Definition =
                $crate::event::Definition { r#type: $type, durable: $durable };
        }
    };
}
pub(crate) use session_event;

/// `session.next.event.source`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub start: NonNegativeInt,
    pub end: NonNegativeInt,
    pub text: String,
}

session_event!(AgentSwitched = "session.next.agent.switched", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub agent: String,
});

session_event!(ModelSwitched = "session.next.model.switched", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub model: ModelRef,
});

session_event!(Moved = "session.next.moved", V1, {
    pub location: location::Ref,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]
    pub subdirectory: Option<RelativePath>,
});

session_event!(Prompted = "session.next.prompted", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub prompt: Prompt,
    pub delivery: Delivery,
});

session_event!(PromptAdmitted = "session.next.prompt.admitted", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub prompt: Prompt,
    pub delivery: Delivery,
});

session_event!(ContextUpdated = "session.next.context.updated", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub text: String,
});

session_event!(Synthetic = "session.next.synthetic", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    pub text: String,
});

session_event!(ShellStarted = "session.next.shell.started", V1, {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub command: String,
});

session_event!(ShellEnded = "session.next.shell.ended", V1, {
    #[serde(rename = "callID")]
    pub call_id: String,
    pub output: String,
});

macro_rules! definitions {
    ($($ty:ty),+ $(,)?) => {
        &[$(<$ty as $crate::event::EventData>::DEFINITION),+]
    };
}

/// `SessionEvent.DurableDefinitions`, baseline order.
pub const DURABLE_DEFINITIONS: &[Definition] = definitions!(
    AgentSwitched,
    ModelSwitched,
    Moved,
    Prompted,
    PromptAdmitted,
    ContextUpdated,
    Synthetic,
    ShellStarted,
    ShellEnded,
    StepStarted,
    StepEnded,
    StepFailed,
    TextStarted,
    TextEnded,
    ToolInputStarted,
    ToolInputEnded,
    ToolCalled,
    ToolProgress,
    ToolSuccess,
    ToolFailed,
    ReasoningStarted,
    ReasoningEnded,
    Retried,
    CompactionStarted,
    CompactionEnded,
    RevertStaged,
    RevertCleared,
    RevertCommitted,
);

/// `SessionEvent.Definitions`, baseline order (includes live-only deltas).
pub const DEFINITIONS: &[Definition] = definitions!(
    AgentSwitched,
    ModelSwitched,
    Moved,
    Prompted,
    PromptAdmitted,
    ContextUpdated,
    Synthetic,
    ShellStarted,
    ShellEnded,
    StepStarted,
    StepEnded,
    StepFailed,
    TextStarted,
    TextDelta,
    TextEnded,
    ReasoningStarted,
    ReasoningDelta,
    ReasoningEnded,
    ToolInputStarted,
    ToolInputDelta,
    ToolInputEnded,
    ToolCalled,
    ToolProgress,
    ToolSuccess,
    ToolFailed,
    Retried,
    CompactionStarted,
    CompactionDelta,
    CompactionEnded,
    RevertStaged,
    RevertCleared,
    RevertCommitted,
);

// `tagged_union!` matches on literal patterns, so the unions below spell out each type string.
tagged_union!(
    /// `SessionDurableEvent` (`SessionEvent.Durable`).
    DurableEvent, tag = "type", expected = "SessionDurableEvent", {
        AgentSwitched(Event<AgentSwitched>) = "session.next.agent.switched",
        ModelSwitched(Event<ModelSwitched>) = "session.next.model.switched",
        Moved(Event<Moved>) = "session.next.moved",
        Prompted(Event<Prompted>) = "session.next.prompted",
        PromptAdmitted(Event<PromptAdmitted>) = "session.next.prompt.admitted",
        ContextUpdated(Event<ContextUpdated>) = "session.next.context.updated",
        Synthetic(Event<Synthetic>) = "session.next.synthetic",
        ShellStarted(Event<ShellStarted>) = "session.next.shell.started",
        ShellEnded(Event<ShellEnded>) = "session.next.shell.ended",
        StepStarted(Event<StepStarted>) = "session.next.step.started",
        StepEnded(Event<StepEnded>) = "session.next.step.ended",
        StepFailed(Event<StepFailed>) = "session.next.step.failed",
        TextStarted(Event<TextStarted>) = "session.next.text.started",
        TextEnded(Event<TextEnded>) = "session.next.text.ended",
        ToolInputStarted(Event<ToolInputStarted>) = "session.next.tool.input.started",
        ToolInputEnded(Event<ToolInputEnded>) = "session.next.tool.input.ended",
        ToolCalled(Event<ToolCalled>) = "session.next.tool.called",
        ToolProgress(Event<ToolProgress>) = "session.next.tool.progress",
        ToolSuccess(Event<ToolSuccess>) = "session.next.tool.success",
        ToolFailed(Event<ToolFailed>) = "session.next.tool.failed",
        ReasoningStarted(Event<ReasoningStarted>) = "session.next.reasoning.started",
        ReasoningEnded(Event<ReasoningEnded>) = "session.next.reasoning.ended",
        Retried(Event<Retried>) = "session.next.retried",
        CompactionStarted(Event<CompactionStarted>) = "session.next.compaction.started",
        CompactionEnded(Event<CompactionEnded>) = "session.next.compaction.ended",
        RevertStaged(Event<RevertStaged>) = "session.next.revert.staged",
        RevertCleared(Event<RevertCleared>) = "session.next.revert.cleared",
        RevertCommitted(Event<RevertCommitted>) = "session.next.revert.committed",
    }
);

tagged_union!(
    /// `SessionEvent.All`: every session event including live-only deltas.
    SessionEvent, tag = "type", expected = "SessionEvent", {
        AgentSwitched(Event<AgentSwitched>) = "session.next.agent.switched",
        ModelSwitched(Event<ModelSwitched>) = "session.next.model.switched",
        Moved(Event<Moved>) = "session.next.moved",
        Prompted(Event<Prompted>) = "session.next.prompted",
        PromptAdmitted(Event<PromptAdmitted>) = "session.next.prompt.admitted",
        ContextUpdated(Event<ContextUpdated>) = "session.next.context.updated",
        Synthetic(Event<Synthetic>) = "session.next.synthetic",
        ShellStarted(Event<ShellStarted>) = "session.next.shell.started",
        ShellEnded(Event<ShellEnded>) = "session.next.shell.ended",
        StepStarted(Event<StepStarted>) = "session.next.step.started",
        StepEnded(Event<StepEnded>) = "session.next.step.ended",
        StepFailed(Event<StepFailed>) = "session.next.step.failed",
        TextStarted(Event<TextStarted>) = "session.next.text.started",
        TextDelta(Event<TextDelta>) = "session.next.text.delta",
        TextEnded(Event<TextEnded>) = "session.next.text.ended",
        ReasoningStarted(Event<ReasoningStarted>) = "session.next.reasoning.started",
        ReasoningDelta(Event<ReasoningDelta>) = "session.next.reasoning.delta",
        ReasoningEnded(Event<ReasoningEnded>) = "session.next.reasoning.ended",
        ToolInputStarted(Event<ToolInputStarted>) = "session.next.tool.input.started",
        ToolInputDelta(Event<ToolInputDelta>) = "session.next.tool.input.delta",
        ToolInputEnded(Event<ToolInputEnded>) = "session.next.tool.input.ended",
        ToolCalled(Event<ToolCalled>) = "session.next.tool.called",
        ToolProgress(Event<ToolProgress>) = "session.next.tool.progress",
        ToolSuccess(Event<ToolSuccess>) = "session.next.tool.success",
        ToolFailed(Event<ToolFailed>) = "session.next.tool.failed",
        Retried(Event<Retried>) = "session.next.retried",
        CompactionStarted(Event<CompactionStarted>) = "session.next.compaction.started",
        CompactionDelta(Event<CompactionDelta>) = "session.next.compaction.delta",
        CompactionEnded(Event<CompactionEnded>) = "session.next.compaction.ended",
        RevertStaged(Event<RevertStaged>) = "session.next.revert.staged",
        RevertCleared(Event<RevertCleared>) = "session.next.revert.cleared",
        RevertCommitted(Event<RevertCommitted>) = "session.next.revert.committed",
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{durable, latest};

    #[test]
    fn inventories_match_baseline_counts() {
        // Observed baseline: DurableDefinitions 28, Definitions 32.
        assert_eq!((DURABLE_DEFINITIONS.len(), DEFINITIONS.len()), (28, 32));
        assert_eq!(durable(DURABLE_DEFINITIONS).unwrap().len(), 28);
        assert_eq!(latest(DEFINITIONS).unwrap().len(), 32);
        assert!(
            durable(DEFINITIONS)
                .unwrap()
                .contains_key("session.next.step.ended.2")
        );
    }

    #[test]
    fn durable_union_rejects_live_only_delta() {
        // fixture: captured from bun at baseline. text.delta is not a SessionDurableEvent.
        let delta = r#"{"type":"session.next.text.delta","id":"evt_1","data":{"sessionID":"ses_1","timestamp":1,"assistantMessageID":"msg_1","textID":"t","delta":"d"}}"#;
        assert!(serde_json::from_str::<DurableEvent>(delta).is_err());
        let all: SessionEvent = serde_json::from_str(delta).unwrap();
        assert_eq!(
            serde_json::to_string(&all).unwrap(),
            r#"{"id":"evt_1","type":"session.next.text.delta","data":{"timestamp":1,"sessionID":"ses_1","assistantMessageID":"msg_1","textID":"t","delta":"d"}}"#
        );
    }
}
