//! Purpose: Session history selection (`packages/core/src/session/history.ts`): latest compaction,
//! the compaction window plus post-baseline system messages, returned as raw `session_message`
//! rows because `context` may not depend on `session-api`.
//! Scope: synchronous queries on one `Sql` unit of work. Decoding into `Message` is session-runtime's
//! (`Message::from_row` over [`MessageRow::message_json`]).
//! Uses:
//! rust/crates/
//! |-- context/src/source/mod.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/src/lib.rs
//! |-- context/tests/epoch.rs
//! `-- context/tests/history.rs
//! Invariants: rows ascend by seq; with a compaction only seq >= compaction (plus system messages
//! newer than the baseline) survive; system messages at or before the baseline are always hidden
//! because the baseline already contains them.
//! Contract tests: context/tests/epoch.rs, context/tests/history.rs

use opencode_storage_api::{Row, Sql, Value};
use serde_json::Map;

use crate::source::ContextError;

/// One raw `session_message` row.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub message_type: String,
    pub seq: i64,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: serde_json::Value,
}

impl MessageRow {
    fn from_row(row: &Row) -> Result<Self, ContextError> {
        Ok(Self {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            message_type: row.get("type")?,
            seq: row.get("seq")?,
            time_created: row.get("time_created")?,
            time_updated: row.get("time_updated")?,
            data: row.get_json("data")?,
        })
    }

    /// The decode input of the baseline: `{ ...row.data, id: row.id, type: row.type }`.
    pub fn message_json(&self) -> serde_json::Value {
        let mut object = match &self.data {
            serde_json::Value::Object(map) => map.clone(),
            _ => Map::new(),
        };
        object.insert("id".into(), self.id.clone().into());
        object.insert("type".into(), self.message_type.clone().into());
        serde_json::Value::Object(object)
    }
}

/// Runner entry: the row plus its seq (`entriesForRunner`).
pub type Entry = MessageRow;

/// Seq of the newest compaction message, if any.
pub fn latest_compaction(sql: &dyn Sql, session_id: &str) -> Result<Option<i64>, ContextError> {
    let row = sql.query_row(
        "SELECT seq FROM session_message WHERE session_id = ? AND type = 'compaction' ORDER BY seq DESC LIMIT 1",
        &[Value::from(session_id)],
    )?;
    Ok(row.map(|r| r.get::<i64>("seq")).transpose()?)
}

/// `baseline_seq` of the Session's context epoch, if initialized.
pub fn baseline_seq(sql: &dyn Sql, session_id: &str) -> Result<Option<i64>, ContextError> {
    let row = sql.query_row(
        "SELECT baseline_seq FROM session_context_epoch WHERE session_id = ?",
        &[Value::from(session_id)],
    )?;
    Ok(row.map(|r| r.get::<i64>("baseline_seq")).transpose()?)
}

/// Visible history for clients (`SessionHistory.load`).
pub fn load(sql: &dyn Sql, session_id: &str) -> Result<Vec<MessageRow>, ContextError> {
    let baseline = baseline_seq(sql, session_id)?;
    let compaction = latest_compaction(sql, session_id)?;
    message_rows(sql, session_id, compaction, baseline)
}

/// Runner history after the given baseline (`SessionHistory.entriesForRunner`).
pub fn entries_for_runner(
    sql: &dyn Sql,
    session_id: &str,
    baseline_seq: i64,
) -> Result<Vec<Entry>, ContextError> {
    let compaction = latest_compaction(sql, session_id)?;
    message_rows(sql, session_id, compaction, Some(baseline_seq))
}

/// The selection query (`messageRows`).
pub fn message_rows(
    sql: &dyn Sql,
    session_id: &str,
    compaction: Option<i64>,
    baseline_seq: Option<i64>,
) -> Result<Vec<MessageRow>, ContextError> {
    let mut query = String::from(
        "SELECT id, session_id, type, seq, time_created, time_updated, data FROM session_message WHERE session_id = ?",
    );
    let mut params = vec![Value::from(session_id)];
    if let Some(compaction) = compaction {
        match baseline_seq {
            Some(baseline) => {
                query.push_str(" AND (seq >= ? OR (type = 'system' AND seq > ?))");
                params.extend([Value::Integer(compaction), Value::Integer(baseline)]);
            }
            None => {
                query.push_str(" AND seq >= ?");
                params.push(Value::Integer(compaction));
            }
        }
    }
    if let Some(baseline) = baseline_seq {
        query.push_str(" AND (type != 'system' OR seq > ?)");
        params.push(Value::Integer(baseline));
    }
    query.push_str(" ORDER BY seq ASC");
    sql.query(&query, &params)?
        .iter()
        .map(MessageRow::from_row)
        .collect()
}
