//! Purpose: public share Durable Object (`SyncServer`) rules ported from packages/function/src/api.ts:16-204:
//! short names, publish key validation, R2 object keys, replay filtering, share_data assembly and
//! the clear-prefix behavior.
//! Scope: pure decisions over an explicit state snapshot. The host (DO runtime or in-process
//! actor) performs the storage, bucket and WebSocket effects in the order returned here.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/sharing/mod.rs
//! `-- hosted-domain/src/sharing/share.rs
//! External:
//! |-- serde::Serialize
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/public.rs
//! `-- hosted-domain/tests/sharing_domain.rs
//! Invariants: publish writes bucket `share/<key>.json`, then DO storage, then broadcasts
//! `{"key","content"}` (api.ts:46-66). `clear` lists `session/message/<sid>/` (limit 1000) and
//! deletes `session/info/<sid>` without `.json`, i.e. NOT the keys publish wrote (api.ts:94-106,
//! faithful defect, H13-revoke). An unset session ID stringifies as `undefined`.
//! Contract tests: hosted-domain/tests/sharing_domain.rs, hosted-adapters/tests/sharing_public.rs

use serde::Serialize;
use serde_json::{Map, Value};

use crate::sharing::share::last_utf16;

/// `SyncServer.shortName` (api.ts:109-111): last 8 UTF-16 units.
pub fn short_name(session_id: &str) -> String {
    last_utf16(session_id, 8)
}

fn sid(session_id: Option<&str>) -> &str {
    session_id.unwrap_or("undefined")
}

/// Publish key check (api.ts:47-53). Note `session/info/<sid>` has no trailing slash, so any key
/// that merely starts with it (e.g. `session/info/<sid>extra`) is accepted.
pub fn publish_allowed(session_id: Option<&str>, key: &str) -> bool {
    let sid = sid(session_id);
    key.starts_with(&format!("session/info/{sid}"))
        || key.starts_with(&format!("session/message/{sid}/"))
        || key.starts_with(&format!("session/part/{sid}/"))
}

/// Bucket object written by publish (api.ts:56).
pub fn publish_object_key(key: &str) -> String {
    format!("share/{key}.json")
}

/// One WebSocket frame / getData entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub key: String,
    pub content: Value,
}

/// Replay and getData filter (api.ts:29-32, 80-84): DO storage entries under `session/`, in
/// storage list order (lexicographic by key in the DO runtime).
pub fn replay(storage: &[(String, Value)]) -> Vec<Entry> {
    storage
        .iter()
        .filter(|(key, _)| key.starts_with("session/"))
        .map(|(key, content)| Entry {
            key: key.clone(),
            content: content.clone(),
        })
        .collect()
}

/// Bucket operations performed by `clear` (api.ts:94-106).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearPlan {
    pub list_prefix: String,
    pub list_limit: u32,
    pub delete_info_key: String,
}

pub fn clear_plan(session_id: Option<&str>) -> ClearPlan {
    let sid = sid(session_id);
    ClearPlan {
        list_prefix: format!("session/message/{sid}/"),
        list_limit: 1000,
        delete_info_key: format!("session/info/{sid}"),
    }
}

/// Failure of the `/share_data` assembly that the baseline surfaces as an uncaught TypeError.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyError(pub String);

/// `/share_data` response body (api.ts:176-203): `{info, messages}` where `info` is omitted when
/// absent, messages are keyed by `content.id` with `parts: []` first then message fields, and
/// parts are appended to their message. A part whose message has not been seen throws.
pub fn share_data(entries: &[Entry]) -> Result<Value, AssemblyError> {
    let mut info: Option<Value> = None;
    let mut messages: Map<String, Value> = Map::new();
    for entry in entries {
        let mut segments = entry.key.split('/');
        if segments.next() != Some("session") {
            continue;
        }
        match segments.next() {
            Some("info") => info = Some(entry.content.clone()),
            Some("message") => {
                let id = crate::sharing::share::js_string(entry.content.get("id"));
                let mut message = Map::new();
                message.insert("parts".into(), Value::Array(Vec::new()));
                if let Value::Object(fields) = &entry.content {
                    for (k, v) in fields {
                        message.insert(k.clone(), v.clone());
                    }
                }
                // Reassigning an existing JS object key keeps its original position, as does
                // IndexMap::insert. Integer-like keys would sort first in JS, message IDs are not.
                messages.insert(id, Value::Object(message));
            }
            Some("part") => {
                let message_id = crate::sharing::share::js_string(entry.content.get("messageID"));
                let parts = messages
                    .get_mut(&message_id)
                    .and_then(|m| m.get_mut("parts"))
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| {
                        AssemblyError(format!("message {message_id} not found for part"))
                    })?;
                parts.push(entry.content.clone());
            }
            _ => {}
        }
    }
    let mut body = Map::new();
    if let Some(info) = info {
        body.insert("info".into(), info);
    }
    body.insert("messages".into(), Value::Object(messages));
    Ok(Value::Object(body))
}
