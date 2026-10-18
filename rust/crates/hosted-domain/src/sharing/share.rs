//! Purpose: enterprise share formats and pure rules ported from packages/enterprise/src/core/share.ts
//! and storage.ts: share info, data union, keyed latest-wins merge, short-ID creation, legacy
//! compaction import, object key layout and removal prefixes.
//! Scope: pure values and decisions. Effects (object reads/writes) live in hosted-adapters.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/sharing/collation.rs
//! `-- hosted-domain/src/sharing/mod.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/service.rs
//! |-- hosted-domain/src/sharing/mod.rs
//! `-- hosted-domain/tests/sharing_domain.rs
//! Invariants: data items serialize as exactly `{"type","data"}` (zod strips unknown keys),
//! merge is latest-wins per key then sorted with `locale_compare` (share.ts:66-76), legacy import
//! lists events *before* the compaction cursor and processes them newest-first (share.ts:86-115,
//! faithful defect), removal lists `<group>/<id>/` prefixes so the `share_snapshot/<id>.json`
//! object survives removal (share.ts:134-148, faithful defect H13-revoke).
//! Contract tests: hosted-domain/tests/sharing_domain.rs, hosted-adapters/tests/sharing_baseline.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sharing::ShareError;
use crate::sharing::collation::locale_compare;

/// `Share.Info` (share.ts:11-16). Field order is the stored JSON order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareInfo {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Session,
    Message,
    Part,
    SessionDiff,
    Model,
}

/// One `Share.Data` union member. `data` is `z.custom` (unchecked) and may be absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareData {
    #[serde(rename = "type")]
    pub kind: DataKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Snapshot object `share_snapshot/<id>.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub data: Vec<ShareData>,
}

/// Compaction object `share_compaction/<id>.json`. `event` is omitted when undefined.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub data: Vec<ShareData>,
}

/// JavaScript template-literal stringification of a JSON value (`${value}`).
pub fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

fn field<'a>(item: &'a ShareData, name: &str) -> Result<Option<&'a Value>, ShareError> {
    match &item.data {
        // `undefined.id` / `null.id` throws a TypeError in the baseline.
        None | Some(Value::Null) => {
            Err(ShareError::Internal(format!("cannot read property {name}")))
        }
        Some(Value::Object(map)) => Ok(map.get(name)),
        Some(_) => Ok(None),
    }
}

/// `key(item)` (share.ts:51-64).
pub fn key(item: &ShareData) -> Result<String, ShareError> {
    Ok(match item.kind {
        DataKind::Session => "session".into(),
        DataKind::Message => format!("message/{}", js_string(field(item, "id")?)),
        DataKind::Part => {
            let message = js_string(field(item, "messageID")?);
            format!("part/{message}/{}", js_string(field(item, "id")?))
        }
        DataKind::SessionDiff => "session_diff".into(),
        DataKind::Model => "model".into(),
    })
}

/// `merge(...lists)` (share.ts:66-76): latest wins per key, keys sorted by `localeCompare`.
pub fn merge(lists: &[&[ShareData]]) -> Result<Vec<ShareData>, ShareError> {
    let mut entries: Vec<(String, ShareData)> = Vec::new();
    for list in lists {
        for item in *list {
            let k = key(item)?;
            match entries.iter_mut().find(|(existing, _)| *existing == k) {
                Some(slot) => slot.1 = item.clone(),
                None => entries.push((k, item.clone())),
            }
        }
    }
    // Keys are unique after the map step, so the stable sort never sees ties.
    entries.sort_by(|(a, _), (b, _)| locale_compare(a, b));
    Ok(entries.into_iter().map(|(_, item)| item).collect())
}

/// `Share.create` id rule (share.ts:117-123): last 8 UTF-16 units, `test_` prefix in test mode.
pub fn new_info(session_id: &str, secret: String, node_env_test: bool) -> ShareInfo {
    let is_test = node_env_test || session_id.starts_with("test_");
    let prefix = if is_test { "test_" } else { "" };
    ShareInfo {
        id: format!("{prefix}{}", last_utf16(session_id, 8)),
        session_id: session_id.into(),
        secret,
    }
}

/// `str.slice(-n)` / `substring(len - n)` on UTF-16 code units.
pub fn last_utf16(input: &str, n: usize) -> String {
    let units: Vec<u16> = input.encode_utf16().collect();
    let start = units.len().saturating_sub(n);
    String::from_utf16_lossy(&units[start..])
}

/// Secret check shared by sync/remove (share.ts:162-164, 135-137).
pub fn authorize(stored: Option<&ShareInfo>, id: &str, secret: &str) -> Result<(), ShareError> {
    let share = stored.ok_or_else(|| ShareError::NotFound(id.into()))?;
    if share.secret != secret {
        return Err(ShareError::InvalidSecret(id.into()));
    }
    Ok(())
}

/// Storage key path segments (storage.ts:93-95 appends `.json` and joins with `/`).
pub fn info_key(id: &str) -> Vec<String> {
    vec!["share".into(), id.into()]
}

pub fn snapshot_key(id: &str) -> Vec<String> {
    vec!["share_snapshot".into(), id.into()]
}

pub fn compaction_key(id: &str) -> Vec<String> {
    vec!["share_compaction".into(), id.into()]
}

pub fn event_prefix(id: &str) -> Vec<String> {
    vec!["share_event".into(), id.into()]
}

/// Prefixes listed by `remove` (share.ts:139-144), in order.
pub fn removal_prefixes(id: &str) -> [Vec<String>; 4] {
    [
        "share_snapshot",
        "share_compaction",
        "share_event",
        "share_data",
    ]
    .map(|g| vec![g.to_string(), id.to_string()])
}

/// Legacy import step after listing events (share.ts:91-115). `events` are the listed keys
/// (ascending, as the object list returns them) paired with their decoded contents (`None` for a
/// missing object). Returns `None` when nothing new was listed: the caller rewrites the snapshot
/// from `compaction.data` when non-empty.
pub fn legacy_step(
    compaction: &Compaction,
    events: &[(Vec<String>, Option<Vec<ShareData>>)],
) -> Result<Option<Compaction>, ShareError> {
    if events.is_empty() {
        return Ok(None);
    }
    let reversed: Vec<&(Vec<String>, Option<Vec<ShareData>>)> = events.iter().rev().collect();
    let flat: Vec<ShareData> = reversed
        .iter()
        .flat_map(|(_, items)| items.clone().unwrap_or_default())
        .collect();
    let next = merge(&[&compaction.data, &flat])?;
    let event = reversed
        .last()
        .and_then(|(segments, _)| segments.last().cloned());
    Ok(Some(Compaction { event, data: next }))
}

/// `syncOld` object key for one item (share.ts:184-207): `share_data/<id>/...`.
pub fn old_data_key(share_id: &str, item: &ShareData) -> Result<Vec<String>, ShareError> {
    let base = |rest: &[String]| {
        let mut out = vec!["share_data".to_string(), share_id.to_string()];
        out.extend_from_slice(rest);
        out
    };
    Ok(match item.kind {
        DataKind::Session => base(&["session".into()]),
        DataKind::Message => base(&["message".into(), js_string(field(item, "id")?)]),
        DataKind::Part => base(&[
            "part".into(),
            js_string(field(item, "messageID")?),
            js_string(field(item, "id")?),
        ]),
        DataKind::SessionDiff => base(&["session_diff".into()]),
        DataKind::Model => base(&["model".into()]),
    })
}
