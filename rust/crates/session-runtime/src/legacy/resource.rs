//! Purpose: MCP resource prompt parts (packages/opencode/src/session/prompt.ts:700-788): text
//! contents become synthetic parts; binary blobs are attached as data URLs when the MIME type is
//! supported and the decoded size is at most 10 MiB, otherwise replaced by an omission note.
//! Scope: one file part with `source.type = "resource"`, read through `PromptTools::read_resource`.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- u64::from
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/legacy/mod.rs
//! Invariants: a failed read degrades to a synthetic failure text; a missing resource fails the
//! prompt; an unwired MCP adapter fails typed `NotYetPorted`.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use serde_json::{Value, json};

use super::LegacySession;
use super::parts::Draft;
use super::ports::PortError;
use super::store::object;
use super::types::{LegacyError, Object, str_field};

const MAX_MCP_RESOURCE_BLOB_BYTES: u64 = 10 * 1024 * 1024;
const SUPPORTED_MCP_RESOURCE_ATTACHMENT_MIMES: [&str; 5] = [
    "application/pdf",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
];

fn base64_size(value: &str) -> u64 {
    let trimmed: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    let padding = if trimmed.ends_with("==") {
        2
    } else {
        u64::from(trimmed.ends_with('='))
    };
    ((trimmed.len() as u64 * 3) / 4).saturating_sub(padding)
}

fn format_bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    if value < 1024 * 1024 {
        return format!("{} KB", value.div_ceil(1024));
    }
    format!("{} MB", value.div_ceil(1024 * 1024))
}

impl LegacySession {
    pub(super) async fn resolve_resource(
        &self,
        draft: &Draft<'_>,
        part: &Object,
        source: Object,
    ) -> Result<Vec<Object>, LegacyError> {
        let client = str_field(&source, "clientName").unwrap_or_default();
        let uri = str_field(&source, "uri").unwrap_or_default();
        let filename = part.get("filename").and_then(Value::as_str);
        let display = filename.unwrap_or("undefined");
        let mut pieces = vec![draft.synthetic(format!("Reading MCP resource: {display} ({uri})"))];
        let content = match self.deps.tools.read_resource(client, uri).await {
            Ok(Some(content)) => content,
            Ok(None) => {
                return Err(LegacyError::Unknown(format!(
                    "Resource not found: {client}/{uri}"
                )));
            }
            Err(PortError::NotYetPorted(what)) => {
                return Err(LegacyError::NotYetPorted(what));
            }
            Err(PortError::Failed(message)) => {
                pieces.push(
                    draft.synthetic(format!("Failed to read MCP resource {display}: {message}")),
                );
                return Ok(pieces);
            }
        };
        let items = match content.get("contents") {
            Some(Value::Array(items)) => items.clone(),
            Some(other) => vec![other.clone()],
            None => vec![],
        };
        let part_mime = str_field(part, "mime").unwrap_or_default();
        for item in items.iter().filter_map(Value::as_object) {
            if let Some(text) = item
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                pieces.push(draft.synthetic(text));
                continue;
            }
            let Some(blob) = item
                .get("blob")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
            else {
                continue;
            };
            let mime = item
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or(part_mime);
            let name = item.get("uri").and_then(Value::as_str).or(filename);
            let label = name.unwrap_or(uri);
            let size = base64_size(blob);
            if !SUPPORTED_MCP_RESOURCE_ATTACHMENT_MIMES.contains(&mime) {
                pieces.push(draft.synthetic(format!(
                    "[Binary MCP resource omitted: {label} ({mime}, {}) is not a supported attachment type]",
                    format_bytes(size)
                )));
                continue;
            }
            if size > MAX_MCP_RESOURCE_BLOB_BYTES {
                pieces.push(draft.synthetic(format!(
                    "[Binary MCP resource omitted: {label} ({mime}, {}) exceeds {}]",
                    format_bytes(size),
                    format_bytes(MAX_MCP_RESOURCE_BLOB_BYTES)
                )));
                continue;
            }
            pieces
                .push(draft.synthetic(format!("[Binary MCP resource attached: {label} ({mime})]")));
            let mut file = object(json!({
                "messageID": draft.message,
                "sessionID": draft.session,
                "type": "file",
                "mime": mime,
                "filename": name,
                "url": format!("data:{mime};base64,{blob}"),
            }));
            file.retain(|_, value| !value.is_null());
            pieces.push(file);
        }
        Ok(pieces)
    }
}
