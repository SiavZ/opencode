//! Purpose: project assistant tool content into render tool parts, including the legacy
//! `filePath`/`filediff` adaptations. Port of `packages/app/src/utils/session-message.ts:26-46,
//! 186-366` (`toolPart`, `normalizeToolInput`, `normalizeToolMetadata`).
//! Scope: pure transform.
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/mod.rs
//! `-- view-model/src/source.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- view-model/src/projection/mod.rs
//! Invariants: streaming input parses as JSON object or `{}`; completed output joins text
//! content with newlines; file content becomes attachments `${call}:file:${index}`.
//! Contract tests: view-model/tests/projection.rs

use serde_json::{Value, json};

use super::{PartKind, RenderPart, ToolStatus};
use crate::source::{JsonObject, ToolContent, ToolPart, ToolState};

pub(super) fn tool_part(message_id: &str, tool: &ToolPart) -> RenderPart {
    let start = tool.time.ran.unwrap_or(tool.time.created);
    let end = Some(tool.time.completed.unwrap_or(start));
    let (input, metadata, status, end) = match &tool.state {
        ToolState::Streaming { input } => {
            let parsed = match serde_json::from_str::<Value>(input) {
                Ok(Value::Object(o)) => o,
                _ => JsonObject::new(),
            };
            let status = ToolStatus::Pending { raw: input.clone() };
            (
                normalize_input(&tool.name, parsed),
                JsonObject::new(),
                status,
                None,
            )
        }
        ToolState::Running { input, metadata } => (
            normalize_input(&tool.name, input.clone()),
            normalize_metadata(&tool.name, metadata.clone()),
            ToolStatus::Running,
            None,
        ),
        ToolState::Error {
            input,
            error,
            metadata,
            ..
        } => (
            normalize_input(&tool.name, input.clone()),
            normalize_metadata(&tool.name, metadata.clone().unwrap_or_default()),
            ToolStatus::Error {
                error: error.message.clone(),
            },
            end,
        ),
        ToolState::Completed {
            input,
            content,
            metadata,
        } => {
            let attachments = content
                .iter()
                .enumerate()
                .filter_map(|(i, c)| match c {
                    ToolContent::File { uri, mime, name } => Some(RenderPart {
                        id: format!("{}:file:{i}", tool.id),
                        message_id: message_id.to_owned(),
                        kind: PartKind::File {
                            mime: mime.clone(),
                            filename: name.clone(),
                            url: uri.clone(),
                            mention_path: None,
                        },
                    }),
                    ToolContent::Text { .. } => None,
                })
                .collect();
            let output = content
                .iter()
                .filter_map(|c| match c {
                    ToolContent::Text { text } => Some(text.as_str()),
                    ToolContent::File { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                normalize_input(&tool.name, input.clone()),
                normalize_metadata(&tool.name, metadata.clone().unwrap_or_default()),
                ToolStatus::Completed {
                    output,
                    title: tool.name.clone(),
                    attachments,
                },
                end,
            )
        }
    };
    RenderPart {
        id: tool.id.clone(),
        message_id: message_id.to_owned(),
        kind: PartKind::Tool {
            tool: tool.name.clone(),
            call_id: tool.id.clone(),
            input,
            metadata,
            status,
            start,
            end,
        },
    }
}

/// `edit`/`write` current inputs carry `path`; legacy renderers read `filePath`.
fn normalize_input(name: &str, mut input: JsonObject) -> JsonObject {
    if !matches!(name, "edit" | "write") || input.get("filePath").is_some_and(Value::is_string) {
        return input;
    }
    if let Some(path) = input.get("path").filter(|v| v.is_string()).cloned() {
        input.insert("filePath".into(), path);
    }
    input
}

/// `edit` current metadata lists `files`; legacy renderer reads a single `filediff`.
fn normalize_metadata(name: &str, mut metadata: JsonObject) -> JsonObject {
    if name != "edit" {
        return metadata;
    }
    let file = metadata
        .get("files")
        .and_then(Value::as_array)
        .and_then(|f| f.iter().find(|v| v.is_object()))
        .cloned();
    let Some(file) = file else { return metadata };
    let Some(path) = file.get("file").and_then(Value::as_str) else {
        return metadata;
    };
    let mut diff = JsonObject::new();
    diff.insert("file".into(), json!(path));
    if let Some(patch) = file.get("patch").filter(|p| p.is_string()) {
        diff.insert("patch".into(), patch.clone());
    }
    diff.insert(
        "additions".into(),
        file.get("additions")
            .filter(|v| v.is_number())
            .cloned()
            .unwrap_or(json!(0)),
    );
    diff.insert(
        "deletions".into(),
        file.get("deletions")
            .filter(|v| v.is_number())
            .cloned()
            .unwrap_or(json!(0)),
    );
    metadata.insert("filediff".into(), Value::Object(diff));
    metadata
}
