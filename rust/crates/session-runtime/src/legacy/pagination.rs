//! Purpose: legacy message cursor pagination matching MessageV2.page and its base64url cursor.
//! Scope: persisted V1 messages and parts, one consistent database read.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- base64::Engine
//! |-- serde::Deserialize
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy.rs
//! Invariants: strict (time_created,id) boundary, latest-first SQL and oldest-first wire items.
//! Contract tests: session-runtime/tests/legacy_pagination.rs

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use opencode_storage_api::{DatabaseExt, Value};
use serde::Deserialize;
use serde_json::json;

use super::store::{LegacyStore, hydrate};
use super::types::{LegacyError, WithParts};

#[derive(Debug, Clone, Deserialize)]
pub struct MessageCursor {
    pub id: String,
    pub time: f64,
}

impl MessageCursor {
    pub fn decode(value: &str) -> Result<Self, LegacyError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(value.trim_end_matches('='))
            .map_err(|_| LegacyError::Unknown("Invalid message cursor".into()))?;
        let cursor: Self = serde_json::from_slice(&decoded)
            .map_err(|_| LegacyError::Unknown("Invalid message cursor".into()))?;
        if !cursor.id.starts_with("msg") || !cursor.time.is_finite() || cursor.time < 0.0 {
            return Err(LegacyError::Unknown("Invalid message cursor".into()));
        }
        Ok(cursor)
    }

    fn encode(&self) -> String {
        let time = if self.time.fract() == 0.0 {
            json!(self.time as i64)
        } else {
            json!(self.time)
        };
        URL_SAFE_NO_PAD.encode(json!({"id":self.id,"time":time}).to_string())
    }
}

#[derive(Debug)]
pub struct MessagePage {
    pub items: Vec<WithParts>,
    pub cursor: Option<String>,
}

impl LegacyStore {
    pub async fn page(
        &self,
        session: &str,
        limit: usize,
        before: Option<MessageCursor>,
    ) -> Result<MessagePage, LegacyError> {
        let id = session.to_owned();
        self.events()
            .database()
            .read(move |tx| -> Result<_, opencode_storage_api::StorageError> {
                let mut parameters = vec![Value::from(id.as_str())];
                let boundary = match before {
                    Some(cursor) => {
                        parameters.extend([
                            Value::Real(cursor.time),
                            Value::Real(cursor.time),
                            Value::from(cursor.id),
                        ]);
                        " AND (time_created < ? OR (time_created = ? AND id < ?))"
                    }
                    None => "",
                };
                parameters.push(Value::Integer(limit.saturating_add(1).min(i64::MAX as usize) as i64));
                let mut rows = tx.query(
                    &format!("SELECT id, session_id, time_created, data FROM message WHERE session_id = ?{boundary} ORDER BY time_created DESC, id DESC LIMIT ?"),
                    &parameters,
                )?;
                if rows.is_empty() && tx.query_row("SELECT id FROM session WHERE id = ?", &[id.as_str().into()])?.is_none() {
                    return Ok(None);
                }
                let more = rows.len() > limit;
                rows.truncate(limit);
                let cursor = if more {
                    rows.last().map(|row| {
                        Ok::<_, opencode_storage_api::StorageError>(MessageCursor {id:row.get("id")?,time:row.get::<i64>("time_created")? as f64}.encode())
                    }).transpose()?
                } else {
                    None
                };
                rows.reverse();
                Ok(Some(MessagePage { items: hydrate(tx, rows)?, cursor }))
            })
            .await?
            .ok_or_else(|| LegacyError::NotFound(session.to_owned()))
    }
}
