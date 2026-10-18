//! Purpose: legacy Session message storage: `Session.updateMessage/updatePart/removeMessage/
//! removePart/messages/findMessage/get/patch/touch/setTitle/setAgentModel/setPermission/setShare/
//! setSummary/clearRevert` (packages/opencode/src/session/session.ts:540-936) over the `message`,
//! `part` and `session` tables. V1 projectors live in `legacy/projectors.rs`.
//! Scope: one process-global `LegacyStore` over `SessionEvents`; ephemeral events go to the bus only.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- opencode_event_api::{Definition, PublishOptions}
//! `-- serde_json::{Map, Value, json}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! `-- session-runtime/tests/legacy_shell.rs
//! Invariants: messages read oldest first, ordered by `(time_created, id)`, parts by `id`.
//! `patch` merges `time`/`share` objects and removes keys patched with `null`.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use std::sync::Arc;

use opencode_event_api::{Definition, PublishOptions};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, TxOptions, Value as Sv};
use serde_json::{Map, Value, json};

use super::types::{LegacyError, Object, WithParts, now_millis, str_field};
use crate::projection::SessionEvents;

fn text(object: &Object, key: &str) -> String {
    str_field(object, key).unwrap_or_default().to_owned()
}

pub const MESSAGE_UPDATED: Definition = Definition::durable("message.updated", 1, "sessionID");
pub const MESSAGE_REMOVED: Definition = Definition::durable("message.removed", 1, "sessionID");
pub const PART_UPDATED: Definition = Definition::durable("message.part.updated", 1, "sessionID");
pub const PART_REMOVED: Definition = Definition::durable("message.part.removed", 1, "sessionID");
pub const SESSION_UPDATED: Definition = Definition::durable("session.updated", 1, "sessionID");
pub const PART_DELTA: Definition = Definition::ephemeral("message.part.delta");
pub const SESSION_ERROR: Definition = Definition::ephemeral("session.error");
pub const SESSION_DIFF: Definition = Definition::ephemeral("session.diff");
pub const SESSION_STATUS: Definition = Definition::ephemeral("session.status");
pub const SESSION_IDLE: Definition = Definition::ephemeral("session.idle");
pub const SESSION_COMPACTED: Definition = Definition::ephemeral("session.compacted");
pub const COMMAND_EXECUTED: Definition = Definition::ephemeral("command.executed");

/// Legacy `Session.Info` as JSON (`fromRow`).
pub type SessionInfo = Object;

/// Legacy Session/message/part operations over one database and event store.
#[derive(Clone)]
pub struct LegacyStore {
    events: Arc<dyn SessionEvents>,
}

impl LegacyStore {
    pub fn new(events: Arc<dyn SessionEvents>) -> Self {
        Self { events }
    }

    pub fn events(&self) -> &Arc<dyn SessionEvents> {
        &self.events
    }

    fn database(&self) -> Arc<dyn Database> {
        self.events.database()
    }

    pub async fn publish(&self, definition: &Definition, data: Value) -> Result<(), LegacyError> {
        self.events
            .store()
            .publish(definition, data, PublishOptions::default())
            .await?;
        Ok(())
    }

    /// `Session.updateMessage`.
    pub async fn update_message(&self, info: &Object) -> Result<(), LegacyError> {
        let session = text(info, "sessionID");
        self.publish(
            &MESSAGE_UPDATED,
            json!({ "sessionID": session, "info": info }),
        )
        .await
    }

    /// `Session.updatePart`.
    pub async fn update_part(&self, part: &Object) -> Result<(), LegacyError> {
        let session = text(part, "sessionID");
        self.publish(
            &PART_UPDATED,
            json!({ "sessionID": session, "part": part, "time": now_millis() }),
        )
        .await
    }

    /// `Session.removeMessage`.
    pub async fn remove_message(&self, session: &str, message: &str) -> Result<(), LegacyError> {
        self.publish(
            &MESSAGE_REMOVED,
            json!({ "sessionID": session, "messageID": message }),
        )
        .await
    }

    /// `Session.removePart`.
    pub async fn remove_part(
        &self,
        session: &str,
        message: &str,
        part: &str,
    ) -> Result<(), LegacyError> {
        self.publish(
            &PART_REMOVED,
            json!({ "sessionID": session, "messageID": message, "partID": part }),
        )
        .await
    }

    /// `session.error` with a `NamedError` object.
    pub async fn publish_error(&self, session: &str, error: Value) -> Result<(), LegacyError> {
        self.publish(
            &SESSION_ERROR,
            json!({ "sessionID": session, "error": error }),
        )
        .await
    }

    /// `Session.get`: `fromRow` JSON or `NotFound`.
    pub async fn get(&self, session: &str) -> Result<SessionInfo, LegacyError> {
        let id = session.to_owned();
        let row = self
            .database()
            .read(move |tx: &dyn Sql| {
                tx.query_row("SELECT * FROM session WHERE id = ?", &[id.into()])
            })
            .await?;
        row.map(|row| from_row(&row))
            .transpose()?
            .ok_or_else(|| LegacyError::NotFound(session.to_owned()))
    }

    /// `Session.messages`: every message oldest first, with parts ordered by id.
    pub async fn messages(&self, session: &str) -> Result<Vec<WithParts>, LegacyError> {
        let id = session.to_owned();
        let result = self
            .database()
            .read(move |tx: &dyn Sql| load_messages(tx, &id, None))
            .await?;
        match result {
            Some(messages) => Ok(messages),
            None => Err(LegacyError::NotFound(session.to_owned())),
        }
    }

    /// `Session.messages({ limit })`: the newest `limit` messages, oldest first.
    pub async fn latest(&self, session: &str, limit: usize) -> Result<Vec<WithParts>, LegacyError> {
        let id = session.to_owned();
        let result = self
            .database()
            .read(move |tx: &dyn Sql| load_messages(tx, &id, Some(limit)))
            .await?;
        result.ok_or_else(|| LegacyError::NotFound(session.to_owned()))
    }

    /// `Session.findMessage`: newest-first search.
    pub async fn find_message(
        &self,
        session: &str,
        predicate: impl Fn(&WithParts) -> bool,
    ) -> Result<Option<WithParts>, LegacyError> {
        Ok(self
            .messages(session)
            .await?
            .into_iter()
            .rev()
            .find(|message| predicate(message)))
    }

    /// `Session.patch` + `session.updated`: merges `patch` into the current info. `null` values
    /// remove the key (the baseline `null` → `undefined` rule for share/summary/revert/permission).
    pub async fn patch(&self, session: &str, patch: Object) -> Result<(), LegacyError> {
        let mut info = self.get(session).await?;
        for (key, value) in patch {
            match (key.as_str(), value) {
                (_, Value::Null) => {
                    info.shift_remove(&key);
                }
                ("time", Value::Object(time)) => {
                    let current = info
                        .entry("time")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(current) = current {
                        for (field, value) in time {
                            current.insert(field, value);
                        }
                    }
                }
                ("share", Value::Object(share)) => {
                    let current = info
                        .entry("share")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(current) = current {
                        for (field, value) in share {
                            current.insert(field, value);
                        }
                    }
                }
                (_, value) => {
                    info.insert(key, value);
                }
            }
        }
        self.publish(
            &SESSION_UPDATED,
            json!({ "sessionID": session, "info": info }),
        )
        .await
    }

    /// `Session.touch`.
    pub async fn touch(&self, session: &str) -> Result<(), LegacyError> {
        self.patch(
            session,
            object(json!({ "time": { "updated": now_millis() } })),
        )
        .await
    }

    /// `Session.setTitle`.
    pub async fn set_title(&self, session: &str, title: &str) -> Result<(), LegacyError> {
        self.patch(session, object(json!({ "title": title }))).await
    }

    /// `Session.setAgentModel`.
    pub async fn set_agent_model(
        &self,
        session: &str,
        agent: &str,
        model: Value,
        time: i64,
    ) -> Result<(), LegacyError> {
        self.patch(
            session,
            object(json!({ "agent": agent, "model": model, "time": { "updated": time } })),
        )
        .await
    }

    /// `Session.setPermission`.
    pub async fn set_permission(
        &self,
        session: &str,
        permission: Value,
    ) -> Result<(), LegacyError> {
        self.patch(
            session,
            object(json!({ "permission": permission, "time": { "updated": now_millis() } })),
        )
        .await
    }

    /// `Session.setShare`. `None` clears the share.
    pub async fn set_share(&self, session: &str, url: Option<&str>) -> Result<(), LegacyError> {
        let share = url.map_or(Value::Null, |url| json!({ "url": url }));
        self.patch(
            session,
            object(json!({ "share": share, "time": { "updated": now_millis() } })),
        )
        .await
    }

    /// `Session.setSummary`.
    pub async fn set_summary(&self, session: &str, summary: Value) -> Result<(), LegacyError> {
        self.patch(
            session,
            object(json!({ "time": { "updated": now_millis() }, "summary": summary })),
        )
        .await
    }

    /// `Session.clearRevert`.
    pub async fn clear_revert(&self, session: &str) -> Result<(), LegacyError> {
        self.patch(
            session,
            object(json!({ "time": { "updated": now_millis() }, "revert": null })),
        )
        .await
    }

    /// `SessionTable.model` of the Session (`currentModel` first step).
    pub async fn session_model(&self, session: &str) -> Result<Option<Value>, LegacyError> {
        let id = session.to_owned();
        let row = self
            .database()
            .read(move |tx: &dyn Sql| {
                tx.query_row("SELECT model FROM session WHERE id = ?", &[id.into()])
            })
            .await?;
        Ok(match row {
            Some(row) => row.get_json_opt::<Value>("model")?,
            None => None,
        })
    }

    /// Writes rows directly in one transaction (test seeding and migrations only).
    pub async fn transaction<T: Send + 'static>(
        &self,
        work: impl FnOnce(&dyn Sql) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, LegacyError> {
        Ok(self
            .database()
            .transaction(TxOptions::default(), work)
            .await?)
    }
}

pub fn object(value: Value) -> Object {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn load_messages(
    tx: &dyn Sql,
    session: &str,
    limit: Option<usize>,
) -> Result<Option<Vec<WithParts>>, StorageError> {
    let mut rows = match limit {
        Some(limit) => tx.query(
            "SELECT id, session_id, data FROM message WHERE session_id = ? ORDER BY time_created DESC, id DESC LIMIT ?",
            &[session.into(), Sv::Integer(limit as i64)],
        )?,
        None => tx.query(
            "SELECT id, session_id, data FROM message WHERE session_id = ? ORDER BY time_created DESC, id DESC",
            &[session.into()],
        )?,
    };
    if rows.is_empty() {
        let exists = tx
            .query_row("SELECT id FROM session WHERE id = ?", &[session.into()])?
            .is_some();
        return Ok(exists.then(Vec::new));
    }
    rows.reverse();
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id")?;
        let mut info = Map::new();
        if let Value::Object(data) = row.get_json::<Value>("data")? {
            info.extend(data);
        }
        info.insert("id".into(), Value::String(id.clone()));
        info.insert("sessionID".into(), Value::String(row.get("session_id")?));
        let parts = tx
            .query(
                "SELECT id, session_id, message_id, data FROM part WHERE message_id = ? ORDER BY id",
                &[id.into()],
            )?
            .into_iter()
            .map(|row| {
                let mut part = Map::new();
                if let Value::Object(data) = row.get_json::<Value>("data")? {
                    part.extend(data);
                }
                part.insert("id".into(), Value::String(row.get("id")?));
                part.insert("sessionID".into(), Value::String(row.get("session_id")?));
                part.insert("messageID".into(), Value::String(row.get("message_id")?));
                Ok(part)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        result.push(WithParts { info, parts });
    }
    Ok(Some(result))
}

/// `Session.fromRow` (session.ts:59-118), absent optionals omitted.
fn from_row(row: &opencode_storage_api::Row) -> Result<Object, StorageError> {
    let text = |column: &str| row.get::<Option<String>>(column);
    let int = |column: &str| row.get::<Option<i64>>(column);
    let mut info = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            info.insert(key.into(), value);
        }
    };
    put("id", text("id")?.map(Value::String));
    put("slug", text("slug")?.map(Value::String));
    put("projectID", text("project_id")?.map(Value::String));
    put("workspaceID", text("workspace_id")?.map(Value::String));
    put("directory", text("directory")?.map(Value::String));
    put("path", text("path")?.map(Value::String));
    put("parentID", text("parent_id")?.map(Value::String));
    put("title", text("title")?.map(Value::String));
    put("agent", text("agent")?.map(Value::String));
    put("model", row.get_json_opt::<Value>("model")?);
    put("version", text("version")?.map(Value::String));
    let (additions, deletions, files) = (
        int("summary_additions")?,
        int("summary_deletions")?,
        int("summary_files")?,
    );
    if additions.is_some() || deletions.is_some() || files.is_some() {
        let mut summary = Map::new();
        summary.insert("additions".into(), json!(additions.unwrap_or(0)));
        summary.insert("deletions".into(), json!(deletions.unwrap_or(0)));
        summary.insert("files".into(), json!(files.unwrap_or(0)));
        if let Some(diffs) = row.get_json_opt::<Value>("summary_diffs")? {
            summary.insert("diffs".into(), diffs);
        }
        put("summary", Some(Value::Object(summary)));
    }
    put("cost", Some(json!(row.get::<f64>("cost")?)));
    put(
        "tokens",
        Some(json!({
            "input": row.get::<f64>("tokens_input")?,
            "output": row.get::<f64>("tokens_output")?,
            "reasoning": row.get::<f64>("tokens_reasoning")?,
            "cache": {
                "read": row.get::<f64>("tokens_cache_read")?,
                "write": row.get::<f64>("tokens_cache_write")?,
            },
        })),
    );
    put("share", text("share_url")?.map(|url| json!({ "url": url })));
    put("metadata", row.get_json_opt::<Value>("metadata")?);
    put("revert", row.get_json_opt::<Value>("revert")?);
    put("permission", row.get_json_opt::<Value>("permission")?);
    let mut time = Map::new();
    time.insert("created".into(), json!(row.get::<i64>("time_created")?));
    time.insert("updated".into(), json!(row.get::<i64>("time_updated")?));
    if let Some(compacting) = int("time_compacting")? {
        time.insert("compacting".into(), json!(compacting));
    }
    if let Some(archived) = row.get::<Option<f64>>("time_archived")? {
        time.insert("archived".into(), json!(archived));
    }
    put("time", Some(Value::Object(time)));
    Ok(info)
}
