//! Purpose: `session-status-event.ts` (SessionStatus union, session.status, session.idle),
//! `session-compaction-event.ts` (session.compacted) and `session-todo.ts` (Todo, todo.updated).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/macros.rs
//! |-- schema/src/session_message.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: SessionStatus is a plain union decoded by its `type` literal (tag first in every
//! member); these events are live-only (no durable spec).
//! Contract tests: schema/src/session_aux_event.rs (unit tests), schema/tests/schema_session.rs

use crate::event::{Definition, EventData};
use crate::ids::SessionID;
use crate::session_message::tagged_union;
use crate::values::{NonNegativeInt, optional};
use serde::{Deserialize, Serialize};

crate::literal!(IdleTag = "idle");
crate::literal!(RetryTag = "retry");
crate::literal!(BusyTag = "busy");

/// `SessionStatus` idle member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Idle {
    pub r#type: IdleTag,
}

/// `SessionStatus` retry action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryAction {
    pub reason: String,
    pub provider: String,
    pub title: String,
    pub message: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub link: Option<String>,
}

/// `SessionStatus` retry member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Retry {
    pub r#type: RetryTag,
    pub attempt: NonNegativeInt,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub action: Option<RetryAction>,
    pub next: NonNegativeInt,
}

/// `SessionStatus` busy member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Busy {
    pub r#type: BusyTag,
}

tagged_union!(
    /// `SessionStatus`.
    SessionStatus, tag = "type", expected = "SessionStatus", {
        Idle(Idle) = "idle",
        Retry(Retry) = "retry",
        Busy(Busy) = "busy",
    }
);

/// `session.status` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub status: SessionStatus,
}

impl EventData for Status {
    const DEFINITION: Definition = Definition {
        r#type: "session.status",
        durable: None,
    };
}

/// `session.idle` payload (deprecated in the baseline, still emitted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionIdle {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
}

impl EventData for SessionIdle {
    const DEFINITION: Definition = Definition {
        r#type: "session.idle",
        durable: None,
    };
}

/// `SessionStatusEvent.Definitions`.
pub const STATUS_DEFINITIONS: &[Definition] = &[Status::DEFINITION, SessionIdle::DEFINITION];

/// `session.compacted` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compacted {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
}

impl EventData for Compacted {
    const DEFINITION: Definition = Definition {
        r#type: "session.compacted",
        durable: None,
    };
}

/// `SessionCompactionEvent.Definitions`.
pub const COMPACTION_DEFINITIONS: &[Definition] = &[Compacted::DEFINITION];

/// `Todo` (`SessionTodo.Info`). Status and priority are arbitrary strings in the baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

/// `todo.updated` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoUpdated {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub todos: Vec<Todo>,
}

impl EventData for TodoUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "todo.updated",
        durable: None,
    };
}

/// `SessionTodo.Event.Definitions`.
pub const TODO_DEFINITIONS: &[Definition] = &[TodoUpdated::DEFINITION];

#[cfg(test)]
mod tests {
    use super::*;

    fn rt<T: Serialize + for<'de> Deserialize<'de>>(input: &str) -> String {
        serde_json::to_string(&serde_json::from_str::<T>(input).unwrap()).unwrap()
    }

    #[test]
    fn status_matches_baseline() {
        // fixture: captured from bun at baseline (packages/schema 907b3bc51).
        assert_eq!(
            rt::<SessionStatus>(r#"{"next":3,"message":"m","attempt":1,"type":"retry"}"#),
            r#"{"type":"retry","attempt":1,"message":"m","next":3}"#
        );
        assert_eq!(
            rt::<SessionStatus>(
                r#"{"next":3,"action":{"link":"l","label":"b","message":"m","title":"t","provider":"p","reason":"r"},"message":"m","attempt":1,"type":"retry"}"#
            ),
            r#"{"type":"retry","attempt":1,"message":"m","action":{"reason":"r","provider":"p","title":"t","message":"m","label":"b","link":"l"},"next":3}"#
        );
        assert_eq!(
            rt::<SessionStatus>(r#"{"type":"idle","extra":1}"#),
            r#"{"type":"idle"}"#
        );
    }

    #[test]
    fn status_rejects_invalid() {
        assert!(serde_json::from_str::<SessionStatus>(r#"{"type":"sleeping"}"#).is_err());
        assert!(
            serde_json::from_str::<SessionStatus>(
                r#"{"type":"retry","attempt":-1,"message":"m","next":0}"#
            )
            .is_err()
        );
        assert_eq!(
            (
                STATUS_DEFINITIONS.len(),
                COMPACTION_DEFINITIONS.len(),
                TODO_DEFINITIONS.len()
            ),
            (2, 1, 1)
        );
    }
}
