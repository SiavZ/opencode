//! Purpose: SQL adapter of packages/core/src/session/projector.ts `run`: reads and writes
//! `session_message` rows for the message updater, scoped to one event's Session and durable seq.
//! Scope: synchronous, runs inside the event publish transaction.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/session_message.rs
//! |-- session-api/src/message.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::Error
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::Value::as_str
//! `-- sm::Shell
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/projection.rs
//! `-- session-runtime/src/projection/updater.rs
//! Invariants: only the latest assistant row is resumable, and only while incomplete. Appended rows
//! take the event's durable seq; updates keep the row's seq and refresh `time_updated`.
//! Contract tests: session-runtime/tests/admission_projection.rs

use opencode_event_api::Payload;
use opencode_schema::session_message::{self as sm, Assistant, Message};
use opencode_session_api::message::{created_millis, from_row, to_row};
use opencode_storage_api::{Sql, Value};

use super::{ProjectionError, now_millis};

/// SQL adapter of `SessionProjector.run`, scoped to the event's Session and durable seq.
pub(crate) struct SqlAdapter<'a> {
    tx: &'a dyn Sql,
    session_id: String,
    seq: Option<i64>,
}

impl<'a> SqlAdapter<'a> {
    pub(crate) fn new(tx: &'a dyn Sql, payload: &Payload) -> Result<Self, ProjectionError> {
        let session_id = payload
            .data
            .get("sessionID")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ProjectionError::Decode {
                event_type: payload.event_type.clone(),
                message: "missing sessionID".into(),
            })?
            .to_owned();
        Ok(Self {
            tx,
            session_id,
            seq: payload.durable.as_ref().map(|durable| durable.seq),
        })
    }

    fn decode(&self, row: &opencode_storage_api::Row) -> Result<Message, ProjectionError> {
        let id: String = row.get("id")?;
        let kind: String = row.get("type")?;
        let data: serde_json::Map<String, serde_json::Value> = row.get_json("data")?;
        from_row(&id, &kind, data).map_err(|error| ProjectionError::Decode {
            event_type: format!("session_message {id}"),
            message: error.to_string(),
        })
    }

    pub(crate) fn current_assistant(&self) -> Result<Option<Assistant>, ProjectionError> {
        let row = self.tx.query_row(
            "SELECT id, type, data FROM session_message WHERE session_id = ? AND type = 'assistant' ORDER BY seq DESC LIMIT 1",
            &[self.session_id.as_str().into()],
        )?;
        let Some(row) = row else { return Ok(None) };
        Ok(match self.decode(&row)? {
            Message::Assistant(assistant) if assistant.time.completed.is_none() => Some(assistant),
            _ => None,
        })
    }

    pub(crate) fn assistant(&self, id: &str) -> Result<Option<Assistant>, ProjectionError> {
        let row = self.tx.query_row(
            "SELECT id, type, data FROM session_message WHERE id = ? AND session_id = ? AND type = 'assistant'",
            &[id.into(), self.session_id.as_str().into()],
        )?;
        let Some(row) = row else { return Ok(None) };
        Ok(match self.decode(&row)? {
            Message::Assistant(assistant) => Some(assistant),
            _ => None,
        })
    }

    pub(crate) fn current_shell(
        &self,
        call_id: &str,
    ) -> Result<Option<sm::Shell>, ProjectionError> {
        let rows = self.tx.query(
            "SELECT id, type, data FROM session_message WHERE session_id = ? AND type = 'shell' ORDER BY seq DESC",
            &[self.session_id.as_str().into()],
        )?;
        for row in &rows {
            if let Message::Shell(shell) = self.decode(row)?
                && shell.call_id == call_id
            {
                return Ok(Some(shell));
            }
        }
        Ok(None)
    }

    pub(crate) fn owned(
        &self,
        id: &opencode_schema::MessageID,
        recipe: impl FnOnce(&mut Assistant),
    ) -> Result<(), ProjectionError> {
        let Some(mut assistant) = self.assistant(id.as_str())? else {
            return Ok(());
        };
        recipe(&mut assistant);
        self.update(&Message::Assistant(assistant))
    }

    pub(crate) fn update(&self, message: &Message) -> Result<(), ProjectionError> {
        self.seq.ok_or(ProjectionError::MissingSequence)?;
        let row = to_row(message).map_err(encode_error)?;
        self.tx.execute(
            "UPDATE session_message SET type = ?, time_created = ?, time_updated = ?, data = ? WHERE id = ? AND session_id = ?",
            &[
                row.kind.into(),
                Value::Integer(created_millis(message)),
                Value::Integer(now_millis()),
                Value::json(&row.data)?,
                row.id.into(),
                self.session_id.as_str().into(),
            ],
        )?;
        Ok(())
    }

    pub(crate) fn append(&self, message: Message) -> Result<(), ProjectionError> {
        insert_message(self.tx, &self.session_id, self.seq, &message)
    }
}

/// `insertMessage`: one `session_message` row at the event's durable seq.
pub(crate) fn insert_message(
    tx: &dyn Sql,
    session_id: &str,
    seq: Option<i64>,
    message: &Message,
) -> Result<(), ProjectionError> {
    let seq = seq.ok_or(ProjectionError::MissingSequence)?;
    let row = to_row(message).map_err(encode_error)?;
    tx.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?, ?)",
        &[
            row.id.into(),
            session_id.into(),
            row.kind.into(),
            Value::Integer(seq),
            Value::Integer(created_millis(message)),
            Value::Integer(now_millis()),
            Value::json(&row.data)?,
        ],
    )?;
    Ok(())
}

fn encode_error(error: serde_json::Error) -> ProjectionError {
    ProjectionError::Decode {
        event_type: "session_message encode".into(),
        message: error.to_string(),
    }
}
