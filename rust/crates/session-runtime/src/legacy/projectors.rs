//! Purpose: V1 message/part projectors of packages/core/src/session/projector.ts:260-330
//! (`message.updated/removed`, `message.part.updated/removed`) including step-finish usage
//! accounting on the `session` row.
//! Scope: synchronous, runs inside the event publish transaction.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- opencode_event_api::{Payload, Projector}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/legacy/mod.rs
//! Invariants: message rows store `info` without `id`/`sessionID`, part rows without
//! `id`/`messageID`/`sessionID`; upserts keep `time_created`. Replacing or removing a step-finish
//! part reverses its previous usage before applying the new one.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use std::sync::Arc;

use opencode_event_api::{Payload, Projector};
use opencode_storage_api::{Sql, StorageError, Value as Sv};
use serde_json::{Map, Value};

use super::store::{MESSAGE_REMOVED, MESSAGE_UPDATED, PART_REMOVED, PART_UPDATED};
use super::types::{Object, now_millis, str_field};

/// Registers the V1 message/part projectors (`SessionProjector` legacy half). Call once per store,
/// after `projection::register_projectors`.
pub fn register_projectors(store: &dyn opencode_event_api::EventStore) {
    store.project(&MESSAGE_UPDATED, projector(project_message_updated));
    store.project(&MESSAGE_REMOVED, projector(project_message_removed));
    store.project(&PART_UPDATED, projector(project_part_updated));
    store.project(&PART_REMOVED, projector(project_part_removed));
}

fn projector(
    run: impl Fn(&dyn Sql, &Map<String, Value>) -> Result<(), StorageError> + Send + Sync + 'static,
) -> Projector {
    Arc::new(move |tx: &dyn Sql, payload: &Payload| {
        let data = payload.data.as_object().cloned().unwrap_or_default();
        run(tx, &data).map_err(|error| Box::new(error) as opencode_event_api::BoxError)
    })
}

fn obj(data: &Map<String, Value>, key: &str) -> Object {
    data.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn text(object: &Object, key: &str) -> String {
    str_field(object, key).unwrap_or_default().to_owned()
}

fn without(object: &Object, keys: &[&str]) -> Object {
    object
        .iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn project_message_updated(tx: &dyn Sql, data: &Map<String, Value>) -> Result<(), StorageError> {
    let info = obj(data, "info");
    let created = info
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(Value::as_f64)
        .unwrap_or_default() as i64;
    let body = Sv::json(&without(&info, &["id", "sessionID"]))?;
    tx.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, time_updated = excluded.time_updated",
        &[
            text(&info, "id").into(),
            text(&info, "sessionID").into(),
            Sv::Integer(created),
            Sv::Integer(now_millis()),
            body,
        ],
    )?;
    Ok(())
}

/// `usage(part)`: cost and tokens of a step-finish part.
fn usage(part: &Value) -> Option<(f64, [f64; 5])> {
    if part.get("type")?.as_str()? != "step-finish" {
        return None;
    }
    let cost = part.get("cost")?.as_f64().unwrap_or_default();
    let tokens = part.get("tokens")?;
    let n = |value: Option<&Value>| value.and_then(Value::as_f64).unwrap_or_default();
    let cache = tokens.get("cache");
    Some((
        cost,
        [
            n(tokens.get("input")),
            n(tokens.get("output")),
            n(tokens.get("reasoning")),
            n(cache.and_then(|cache| cache.get("read"))),
            n(cache.and_then(|cache| cache.get("write"))),
        ],
    ))
}

fn apply_usage(
    tx: &dyn Sql,
    session: &str,
    (cost, tokens): (f64, [f64; 5]),
    sign: f64,
) -> Result<(), StorageError> {
    let mut params = vec![Sv::Real(cost * sign)];
    params.extend(tokens.iter().map(|value| Sv::Real(value * sign)));
    params.push(session.into());
    tx.execute(
        "UPDATE session SET cost = cost + ?, tokens_input = tokens_input + ?, tokens_output = tokens_output + ?, \
         tokens_reasoning = tokens_reasoning + ?, tokens_cache_read = tokens_cache_read + ?, \
         tokens_cache_write = tokens_cache_write + ? WHERE id = ?",
        &params,
    )?;
    Ok(())
}

fn project_message_removed(tx: &dyn Sql, data: &Map<String, Value>) -> Result<(), StorageError> {
    let session = data
        .get("sessionID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = data
        .get("messageID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for row in tx.query(
        "SELECT data FROM part WHERE message_id = ? AND session_id = ?",
        &[message.into(), session.into()],
    )? {
        if let Some(previous) = usage(&row.get_json::<Value>("data")?) {
            apply_usage(tx, session, previous, -1.0)?;
        }
    }
    tx.execute(
        "DELETE FROM message WHERE id = ? AND session_id = ?",
        &[message.into(), session.into()],
    )?;
    Ok(())
}

fn project_part_removed(tx: &dyn Sql, data: &Map<String, Value>) -> Result<(), StorageError> {
    let session = data
        .get("sessionID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let part = data
        .get("partID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(row) = tx.query_row(
        "SELECT data FROM part WHERE id = ? AND session_id = ?",
        &[part.into(), session.into()],
    )? && let Some(previous) = usage(&row.get_json::<Value>("data")?)
    {
        apply_usage(tx, session, previous, -1.0)?;
    }
    tx.execute(
        "DELETE FROM part WHERE id = ? AND session_id = ?",
        &[part.into(), session.into()],
    )?;
    Ok(())
}

fn project_part_updated(tx: &dyn Sql, data: &Map<String, Value>) -> Result<(), StorageError> {
    let part = obj(data, "part");
    let id = text(&part, "id");
    let session = text(&part, "sessionID");
    let time = data.get("time").and_then(Value::as_f64).unwrap_or_default() as i64;
    let previous = tx.query_row(
        "SELECT session_id, data FROM part WHERE id = ?",
        &[id.as_str().into()],
    )?;
    tx.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, time_updated = excluded.time_updated",
        &[
            id.as_str().into(),
            text(&part, "messageID").into(),
            session.as_str().into(),
            Sv::Integer(time),
            Sv::Integer(now_millis()),
            Sv::json(&without(&part, &["id", "messageID", "sessionID"]))?,
        ],
    )?;
    if let Some(row) = previous
        && let Some(old) = usage(&row.get_json::<Value>("data")?)
    {
        apply_usage(tx, &row.get::<String>("session_id")?, old, -1.0)?;
    }
    if let Some(next) = usage(&Value::Object(part)) {
        apply_usage(tx, &session, next, 1.0)?;
    }
    Ok(())
}
