//! Purpose: projected Session messages (schema `Session.Message`) and the `session_message`
//! row split/rebuild used by projectors and history reads.
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- opencode_schema::session_message
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- session-api/src/lib.rs
//! Invariants: the row `data` column is the encoded message minus `id` and `type`, key order kept,
//! like the projector's `const { id, type, ...data } = encodeMessage(message)`.
//! Contract tests: session-api/tests/wire.rs

use serde_json::{Map, Value};

pub use opencode_schema::session_message::*;

/// `(id, type, data)` columns of a `session_message` row.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    pub id: String,
    pub kind: String,
    pub data: Map<String, Value>,
}

/// The `type` column value of a message.
pub fn kind(message: &Message) -> &'static str {
    match message {
        Message::AgentSwitched(_) => "agent-switched",
        Message::ModelSwitched(_) => "model-switched",
        Message::User(_) => "user",
        Message::Synthetic(_) => "synthetic",
        Message::System(_) => "system",
        Message::Shell(_) => "shell",
        Message::Assistant(_) => "assistant",
        Message::Compaction(_) => "compaction",
    }
}

/// `message.time.created` in epoch millis.
pub fn created_millis(message: &Message) -> i64 {
    match message {
        Message::AgentSwitched(m) => m.time.created.millis(),
        Message::ModelSwitched(m) => m.time.created.millis(),
        Message::User(m) => m.time.created.millis(),
        Message::Synthetic(m) => m.time.created.millis(),
        Message::System(m) => m.time.created.millis(),
        Message::Shell(m) => m.time.created.millis(),
        Message::Assistant(m) => m.time.created.millis(),
        Message::Compaction(m) => m.time.created.millis(),
    }
}

/// Splits a message into its row columns.
pub fn to_row(message: &Message) -> Result<MessageRow, serde_json::Error> {
    let mut data = match serde_json::to_value(message)? {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    data.shift_remove("id");
    data.shift_remove("type");
    Ok(MessageRow {
        id: message.id().as_str().to_owned(),
        kind: kind(message).to_owned(),
        data,
    })
}

/// Rebuilds a message from row columns, like `decode({ ...row.data, id: row.id, type: row.type })`.
/// Spread order puts row data first, then `id` and `type` override.
pub fn from_row(
    id: &str,
    kind: &str,
    data: Map<String, Value>,
) -> Result<Message, serde_json::Error> {
    let mut value = data;
    value.insert("id".into(), Value::String(id.into()));
    value.insert("type".into(), Value::String(kind.into()));
    serde_json::from_value(Value::Object(value))
}
