//! Purpose: legacy file-change summaries (packages/opencode/src/session/summary.ts):
//! `computeDiff` (first step-start snapshot to last step-finish snapshot), `summarize` (reset Session
//! summary, `session.diff`, per-user-message diffs), `diff` (stored diffs with git C-quoted paths
//! unquoted) and `unquoteGitPath`.
//! Scope: one Session; snapshots through the p05 `Snapshots` port, storage through `LegacyStore`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/snapshot.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/tests/summary.rs
//! Invariants: `summarize` always resets the Session summary to zeros and publishes an empty
//! `session.diff` first; with `snapshot = false` it stops there. Only the target user message is
//! updated, with `summary.diffs` replaced and other summary fields kept.
//! Contract tests: session-runtime/tests/summary.rs

use std::sync::Arc;

use opencode_platform_api::snapshot::Snapshots;
use serde_json::{Value, json};

use crate::legacy::store::{LegacyStore, SESSION_DIFF};
use crate::legacy::types::{LegacyError, WithParts, part_type, str_field};

/// `SessionSummary` over a legacy store and the Location snapshot service.
#[derive(Clone)]
pub struct Summary {
    pub store: LegacyStore,
    pub snapshots: Arc<dyn Snapshots>,
    /// `cfg.snapshot`; `Some(false)` disables per-message diffs.
    pub snapshot: Option<bool>,
}

impl Summary {
    /// `computeDiff`.
    pub async fn compute_diff(&self, messages: &[WithParts]) -> Result<Vec<Value>, LegacyError> {
        let mut from: Option<&str> = None;
        let mut to: Option<&str> = None;
        for message in messages {
            if from.is_none() {
                from = message
                    .parts
                    .iter()
                    .find(|part| part_type(part) == "step-start" && snapshot(part).is_some())
                    .and_then(snapshot);
            }
            for part in &message.parts {
                if part_type(part) == "step-finish"
                    && let Some(value) = snapshot(part)
                {
                    to = Some(value);
                }
            }
        }
        let (Some(from), Some(to)) = (from, to) else {
            return Ok(vec![]);
        };
        let diffs = self
            .snapshots
            .diff(from, to, None, None)
            .await
            .map_err(|error| LegacyError::Unknown(error.to_string()))?;
        Ok(diffs
            .into_iter()
            .map(|diff| serde_json::to_value(diff).unwrap_or(Value::Null))
            .collect())
    }

    /// `summarize({ sessionID, messageID })`.
    pub async fn summarize(&self, session: &str, message: &str) -> Result<(), LegacyError> {
        self.store
            .set_summary(
                session,
                json!({ "additions": 0, "deletions": 0, "files": 0 }),
            )
            .await?;
        self.store
            .publish(&SESSION_DIFF, json!({ "sessionID": session, "diff": [] }))
            .await?;
        if self.snapshot == Some(false) {
            return Ok(());
        }
        let all = self.store.messages(session).await?;
        if all.is_empty() {
            return Ok(());
        }
        let messages: Vec<WithParts> = all
            .into_iter()
            .filter(|m| {
                m.id() == message
                    || (m.is_assistant() && str_field(&m.info, "parentID") == Some(message))
            })
            .collect();
        let Some(target) = messages.iter().find(|m| m.id() == message) else {
            return Ok(());
        };
        if !target.is_user() {
            return Ok(());
        }
        let mut info = target.info.clone();
        let diffs = self.compute_diff(&messages).await?;
        let mut summary = info
            .get("summary")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        summary.insert("diffs".into(), Value::Array(diffs));
        info.insert("summary".into(), Value::Object(summary));
        self.store.update_message(&info).await
    }

    /// `diff({ sessionID, messageID? })`.
    pub async fn diff(
        &self,
        session: &str,
        message: Option<&str>,
    ) -> Result<Vec<Value>, LegacyError> {
        let Some(message) = message else {
            return Ok(vec![]);
        };
        let messages = self.store.messages(session).await?;
        let Some(found) = messages.iter().find(|m| m.id() == message) else {
            return Ok(vec![]);
        };
        if !found.is_user() {
            return Ok(vec![]);
        }
        let diffs = found.info["summary"]["diffs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(diffs
            .into_iter()
            .map(|mut item| {
                if let Some(file) = item.get("file").and_then(Value::as_str) {
                    let unquoted = unquote_git_path(file);
                    if unquoted != file {
                        item["file"] = Value::String(unquoted);
                    }
                }
                item
            })
            .collect())
    }
}

fn snapshot(part: &crate::legacy::types::Object) -> Option<&str> {
    str_field(part, "snapshot").filter(|value| !value.is_empty())
}

/// `unquoteGitPath`: decode a git C-quoted path (`"a\303\251.txt"`) into UTF-8.
pub fn unquote_git_path(input: &str) -> String {
    if !input.starts_with('"') || !input.ends_with('"') || input.len() < 2 {
        return input.to_owned();
    }
    let body: Vec<char> = input[1..input.len() - 1].chars().collect();
    let mut bytes: Vec<u8> = Vec::new();
    let push_char = |bytes: &mut Vec<u8>, ch: char| {
        // `charCodeAt` then `Buffer.from(bytes)`: each UTF-16 unit is truncated to a byte.
        for unit in ch.encode_utf16(&mut [0; 2]) {
            bytes.push(*unit as u8);
        }
    };
    let mut index = 0;
    while index < body.len() {
        let ch = body[index];
        if ch != '\\' {
            push_char(&mut bytes, ch);
            index += 1;
            continue;
        }
        let Some(&next) = body.get(index + 1) else {
            bytes.push(b'\\');
            index += 1;
            continue;
        };
        if ('0'..='7').contains(&next) {
            let digits: String = body[index + 1..]
                .iter()
                .take(3)
                .take_while(|c| ('0'..='7').contains(*c))
                .collect();
            bytes.push(u32::from_str_radix(&digits, 8).unwrap_or(0) as u8);
            index += 1 + digits.len();
            continue;
        }
        let escaped = match next {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'v' => '\u{b}',
            other => other,
        };
        push_char(&mut bytes, escaped);
        index += 2;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
