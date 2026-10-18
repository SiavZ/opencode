//! Purpose: `session-input.ts` (SessionInput.Admitted durable prompt admission record) and
//! `session-delivery.ts` (Delivery: steer or queue).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/prompt.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! `-- schema/src/session_event.rs
//! Invariants: `delivery` is exactly `"steer"` or `"queue"`; sequence numbers are non-negative
//! safe integers; `timeCreated` is epoch millis.
//! Contract tests: schema/src/session_input.rs (unit tests), schema/tests/schema_session.rs

use crate::ids::{MessageID, SessionID};
use crate::prompt::Prompt;
use crate::values::{DateTimeUtc, NonNegativeInt, optional};
use serde::{Deserialize, Serialize};

/// `SessionDelivery.Delivery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Promote at the next safe provider-turn boundary (default).
    #[default]
    Steer,
    /// Stay pending until the session would otherwise become idle.
    Queue,
}

/// `SessionInput.Admitted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Admitted {
    #[serde(rename = "admittedSeq")]
    pub admitted_seq: NonNegativeInt,
    pub id: MessageID,
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub prompt: Prompt,
    pub delivery: Delivery,
    #[serde(rename = "timeCreated")]
    pub time_created: DateTimeUtc,
    #[serde(
        rename = "promotedSeq",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub promoted_seq: Option<NonNegativeInt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admitted_round_trip() {
        let input = r#"{"promotedSeq":2,"timeCreated":1.9,"delivery":"queue","prompt":{"text":"t"},"sessionID":"ses_1","id":"msg_1","admittedSeq":1}"#;
        let admitted: Admitted = serde_json::from_str(input).unwrap();
        assert_eq!(
            serde_json::to_string(&admitted).unwrap(),
            r#"{"admittedSeq":1,"id":"msg_1","sessionID":"ses_1","prompt":{"text":"t"},"delivery":"queue","timeCreated":1,"promotedSeq":2}"#
        );
        let bad = r#"{"timeCreated":1,"delivery":"later","prompt":{"text":"t"},"sessionID":"ses_1","id":"msg_1","admittedSeq":1}"#;
        assert!(serde_json::from_str::<Admitted>(bad).is_err());
        let null = r#"{"timeCreated":1,"delivery":"steer","prompt":{"text":"t"},"sessionID":"ses_1","id":"msg_1","admittedSeq":1,"promotedSeq":null}"#;
        assert!(serde_json::from_str::<Admitted>(null).is_err());
    }
}
