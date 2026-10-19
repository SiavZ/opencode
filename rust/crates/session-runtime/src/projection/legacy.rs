//! Purpose: projectors for the legacy `session.created/updated` events (`sessionRow` in
//! packages/core/src/session/projector.ts), writing the `session` row from a `SessionV1.SessionInfo` payload.
//! Scope: synchronous, runs inside the event publish transaction.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/projection.rs
//! Invariants: `session.created` inserts once (`ON CONFLICT DO NOTHING`) and fails with
//! `SessionAlreadyProjected` when the row exists; `session.updated` only sets columns whose source
//! field is present (Drizzle skips `undefined`), while `workspace_id` and `revert` reset to NULL.
//! Contract tests: session-runtime/tests/admission.rs

use opencode_event_api::Payload;
use opencode_storage_api::{Sql, StorageError, Value};
use serde::Deserialize;
use serde_json::Map;

use super::{ProjectionError, decode, now_millis, session_id};

/// `SessionV1.SessionInfo` columns used by `sessionRow` (legacy `session.created/updated` payload).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1Info {
    id: String,
    slug: String,
    #[serde(rename = "projectID")]
    project_id: String,
    #[serde(rename = "workspaceID")]
    workspace_id: Option<String>,
    directory: String,
    path: Option<String>,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    summary: Option<V1Summary>,
    cost: Option<f64>,
    tokens: Option<V1Tokens>,
    share: Option<V1Share>,
    title: String,
    agent: Option<String>,
    model: Option<serde_json::Value>,
    version: String,
    metadata: Option<Map<String, serde_json::Value>>,
    time: V1Time,
    permission: Option<serde_json::Value>,
    revert: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct V1Summary {
    additions: f64,
    deletions: f64,
    files: f64,
    diffs: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct V1Tokens {
    input: f64,
    output: f64,
    reasoning: f64,
    cache: V1Cache,
}

#[derive(Deserialize)]
struct V1Cache {
    read: f64,
    write: f64,
}

#[derive(Deserialize)]
struct V1Share {
    url: String,
}

#[derive(Deserialize)]
struct V1Time {
    created: i64,
    updated: i64,
    compacting: Option<i64>,
    archived: Option<f64>,
}

#[derive(Deserialize)]
struct V1Event {
    info: V1Info,
}

/// `sessionRow(info)` as (column, value, always_set). Drizzle `.set()` skips `undefined` values, so
/// optional columns are only written when present; `workspace_id` and `revert` default to NULL.
fn session_row(info: &V1Info) -> Result<Vec<(&'static str, Value, bool)>, StorageError> {
    let opt = |value: Option<String>| value.map_or(Value::Null, Value::Text);
    let tokens = info.tokens.as_ref();
    let int = |value: f64| Value::Integer(value as i64);
    Ok(vec![
        ("id", info.id.as_str().into(), true),
        ("project_id", info.project_id.as_str().into(), true),
        ("workspace_id", opt(info.workspace_id.clone()), true),
        (
            "parent_id",
            opt(info.parent_id.clone()),
            info.parent_id.is_some(),
        ),
        ("slug", info.slug.as_str().into(), true),
        ("directory", info.directory.as_str().into(), true),
        ("path", opt(info.path.clone()), info.path.is_some()),
        ("title", info.title.as_str().into(), true),
        ("agent", opt(info.agent.clone()), info.agent.is_some()),
        (
            "model",
            Value::json_opt(info.model.as_ref())?,
            info.model.is_some(),
        ),
        ("version", info.version.as_str().into(), true),
        (
            "share_url",
            opt(info.share.as_ref().map(|s| s.url.clone())),
            info.share.is_some(),
        ),
        (
            "summary_additions",
            info.summary
                .as_ref()
                .map_or(Value::Null, |s| int(s.additions)),
            info.summary.is_some(),
        ),
        (
            "summary_deletions",
            info.summary
                .as_ref()
                .map_or(Value::Null, |s| int(s.deletions)),
            info.summary.is_some(),
        ),
        (
            "summary_files",
            info.summary.as_ref().map_or(Value::Null, |s| int(s.files)),
            info.summary.is_some(),
        ),
        (
            "summary_diffs",
            Value::json_opt(info.summary.as_ref().and_then(|s| s.diffs.as_ref()))?,
            info.summary.as_ref().is_some_and(|s| s.diffs.is_some()),
        ),
        (
            "metadata",
            Value::json_opt(info.metadata.as_ref())?,
            info.metadata.is_some(),
        ),
        ("cost", Value::Real(info.cost.unwrap_or(0.0)), true),
        ("tokens_input", int(tokens.map_or(0.0, |t| t.input)), true),
        ("tokens_output", int(tokens.map_or(0.0, |t| t.output)), true),
        (
            "tokens_reasoning",
            int(tokens.map_or(0.0, |t| t.reasoning)),
            true,
        ),
        (
            "tokens_cache_read",
            int(tokens.map_or(0.0, |t| t.cache.read)),
            true,
        ),
        (
            "tokens_cache_write",
            int(tokens.map_or(0.0, |t| t.cache.write)),
            true,
        ),
        ("revert", Value::json_opt(info.revert.as_ref())?, true),
        (
            "permission",
            Value::json_opt(info.permission.as_ref())?,
            info.permission.is_some(),
        ),
        ("time_created", Value::Integer(info.time.created), true),
        ("time_updated", Value::Integer(info.time.updated), true),
        (
            "time_compacting",
            info.time.compacting.map_or(Value::Null, Value::Integer),
            info.time.compacting.is_some(),
        ),
        (
            "time_archived",
            info.time.archived.map_or(Value::Null, int),
            info.time.archived.is_some(),
        ),
    ])
}

pub(crate) fn project_created(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let event: V1Event = decode(payload)?;
    let row = session_row(&event.info)?;
    let columns: Vec<&str> = row.iter().map(|(column, _, _)| *column).collect();
    let placeholders = vec!["?"; columns.len()].join(", ");
    let values: Vec<Value> = row.into_iter().map(|(_, value, _)| value).collect();
    let stored = tx.query_row(
        &format!(
            "INSERT INTO session ({}) VALUES ({placeholders}) ON CONFLICT DO NOTHING RETURNING id",
            columns.join(", ")
        ),
        &values,
    )?;
    if stored.is_none() {
        return Err(ProjectionError::SessionAlreadyProjected);
    }
    if let Some(workspace) = &event.info.workspace_id {
        tx.execute(
            "UPDATE workspace SET time_used = ? WHERE id = ?",
            &[Value::Integer(now_millis()), workspace.as_str().into()],
        )?;
    }
    Ok(())
}

pub(crate) fn project_updated(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let event: V1Event = decode(payload)?;
    let id = session_id(payload)?;
    let row: Vec<_> = session_row(&event.info)?
        .into_iter()
        .filter(|(_, _, set)| *set)
        .collect();
    let assignments: Vec<String> = row
        .iter()
        .map(|(column, _, _)| format!("{column} = ?"))
        .collect();
    let mut values: Vec<Value> = row.into_iter().map(|(_, value, _)| value).collect();
    values.push(id.into());
    tx.execute(
        &format!("UPDATE session SET {} WHERE id = ?", assignments.join(", ")),
        &values,
    )?;
    Ok(())
}
