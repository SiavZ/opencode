//! Purpose: timeline row and part-group value types with baseline-identical keys. Port of
//! `packages/app/src/pages/session/timeline/timeline-row.ts` and `PartGroup` from
//! `packages/session-ui/src/components/message-part.tsx:622-637`.
//! Scope: data types only.
//! Uses:
//! rust/crates/
//! `-- view-model/src/projection/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- view-model/src/timeline/mod.rs
//! `-- view-model/src/timeline/reconcile.rs
//! Invariants: `TimelineRow::key` matches `TimelineRow.key` byte for byte.
//! Contract tests: view-model/tests/timeline.rs

use crate::projection::SummaryDiff;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartRef {
    pub message_id: String,
    pub part_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PartGroup {
    Part { key: String, part: PartRef },
    Context { key: String, refs: Vec<PartRef> },
}

impl PartGroup {
    pub fn key(&self) -> &str {
        match self {
            Self::Part { key, .. } | Self::Context { key, .. } => key,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DividerLabel {
    Compaction,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TimelineRow {
    TurnGap {
        user_message_id: String,
    },
    CommentStrip {
        user_message_id: String,
    },
    UserMessage {
        user_message_id: String,
        anchor: bool,
    },
    TurnDivider {
        user_message_id: String,
        label: DividerLabel,
    },
    AssistantPart {
        user_message_id: String,
        group: PartGroup,
        previous_assistant_part: bool,
    },
    Thinking {
        user_message_id: String,
        reasoning_heading: Option<String>,
    },
    Retry {
        user_message_id: String,
    },
    DiffSummary {
        user_message_id: String,
        diffs: Vec<SummaryDiff>,
    },
    Error {
        user_message_id: String,
        text: String,
    },
}

impl TimelineRow {
    pub fn user_message_id(&self) -> &str {
        match self {
            Self::TurnGap { user_message_id }
            | Self::CommentStrip { user_message_id }
            | Self::UserMessage {
                user_message_id, ..
            }
            | Self::TurnDivider {
                user_message_id, ..
            }
            | Self::AssistantPart {
                user_message_id, ..
            }
            | Self::Thinking {
                user_message_id, ..
            }
            | Self::Retry { user_message_id }
            | Self::DiffSummary {
                user_message_id, ..
            }
            | Self::Error {
                user_message_id, ..
            } => user_message_id,
        }
    }

    /// Stable row identity, identical to the baseline `TimelineRow.key`.
    pub fn key(&self) -> String {
        let id = self.user_message_id();
        match self {
            Self::TurnGap { .. } => format!("turn-gap:{id}"),
            Self::CommentStrip { .. } => format!("comment-strip:{id}"),
            Self::UserMessage { .. } => format!("user-message:{id}"),
            Self::TurnDivider {
                label: DividerLabel::Compaction,
                ..
            } => format!("turn-divider:{id}:compaction"),
            Self::TurnDivider {
                label: DividerLabel::Interrupted,
                ..
            } => format!("turn-divider:{id}:interrupted"),
            Self::AssistantPart { group, .. } => format!("assistant-part:{id}:{}", group.key()),
            Self::Thinking { .. } => format!("thinking:{id}"),
            Self::DiffSummary { .. } => format!("diff-summary:{id}"),
            Self::Error { .. } => format!("error:{id}"),
            Self::Retry { .. } => format!("retry:{id}"),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::TurnGap { .. } => "TurnGap",
            Self::CommentStrip { .. } => "CommentStrip",
            Self::UserMessage { .. } => "UserMessage",
            Self::TurnDivider { .. } => "TurnDivider",
            Self::AssistantPart { .. } => "AssistantPart",
            Self::Thinking { .. } => "Thinking",
            Self::Retry { .. } => "Retry",
            Self::DiffSummary { .. } => "DiffSummary",
            Self::Error { .. } => "Error",
        }
    }
}
