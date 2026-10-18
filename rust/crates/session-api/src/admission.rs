//! Purpose: durable prompt admission contract: `SessionInput.Admitted` and `Delivery` (schema
//! owned), the exact-retry equivalence rule and the `SessionV2.prompt` request. Does not admit.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- session-api/src/ids.rs
//! `-- session-api/src/prompt.rs
//! External:
//! `-- opencode_schema::session_input
//! Used by:
//! rust/crates/
//! |-- session-api/src/lib.rs
//! `-- session-api/src/operations.rs
//! Invariants: an exact retry matches Session, encoded prompt and delivery (`input.ts:191-214`).
//! Contract tests: session-api/tests/wire.rs

pub use opencode_schema::session_input::{Admitted, Delivery};

use crate::ids::{MessageID, SessionID};
use crate::prompt::{Prompt, PromptInput};

/// `SessionInput.equivalent`: delivery, Session and the JSON encoding of the prompt must match.
/// Encoded comparison mirrors `JSON.stringify(encodePrompt(..))` in the baseline.
pub fn equivalent(
    admitted: &Admitted,
    session_id: &SessionID,
    prompt: &Prompt,
    delivery: Delivery,
) -> bool {
    admitted.delivery == delivery
        && &admitted.session_id == session_id
        && serde_json::to_string(&admitted.prompt).ok() == serde_json::to_string(prompt).ok()
}

/// `Delivery` stored spelling.
pub fn delivery_str(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Steer => "steer",
        Delivery::Queue => "queue",
    }
}

/// Parses the stored `session_input.delivery` column.
pub fn parse_delivery(value: &str) -> Option<Delivery> {
    match value {
        "steer" => Some(Delivery::Steer),
        "queue" => Some(Delivery::Queue),
        _ => None,
    }
}

/// Input of `SessionV2.prompt`.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptRequest {
    /// Caller-chosen prompt message ID. Generated when absent.
    pub id: Option<MessageID>,
    pub session_id: SessionID,
    pub prompt: PromptInput,
    /// Defaults to `steer`.
    pub delivery: Option<Delivery>,
    /// `Some(false)` admits without scheduling a wake.
    pub resume: Option<bool>,
}
