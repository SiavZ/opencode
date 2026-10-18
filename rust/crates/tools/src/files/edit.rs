//! Purpose: Core (V2) exact-text `edit` tool: Location resolution, external then edit approval
//! before any content read, exact occurrence counting, BOM/CRLF preservation, conditional commit.
//! Scope: `packages/core/src/tool/edit.ts` execute, structured output and model-output mapping.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- tool-api/src/context.rs
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/output.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/jsdiff.rs
//! |-- tools/src/files/mutation_input.rs
//! |-- tools/src/files/replacers.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- tools/src/files/mod.rs
//! Invariants: no-op and empty oldString fail before resolution, a denied approval reads zero
//! bytes and reports `Unable to edit <path>` whether or not oldString matches, the write commits
//! only if the bytes read are still on disk (`StaleContent` keeps its own message).
//! Contract tests: tools/tests/files_edit.rs

use super::common::{FileServices, Generation, text_output};
use super::jsdiff::{create_two_files_patch, line_counts};
use super::mutation_input::{CORE_EDIT, core_failure, decode, edit_input};
use super::replacers::js_string_replace;
use super::schemas;
use async_trait::async_trait;
use opencode_platform_api::fs::{MutationError, ResolveKind};
use opencode_tool_api::{Tool, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};

pub const NAME: &str = "edit";

const STALE: &str = "File changed after permission approval. Read it again before editing.";

#[derive(Debug, Clone)]
pub struct CoreEditTool {
    services: FileServices,
}

impl CoreEditTool {
    pub fn new(services: FileServices) -> Self {
        Self { services }
    }
}

#[async_trait]
impl Tool for CoreEditTool {
    fn description(&self) -> &str {
        schemas::description(Generation::Core, NAME)
    }

    fn input_schema(&self) -> Value {
        schemas::input_schema(Generation::Core, NAME)
    }

    fn output_schema(&self) -> Option<Value> {
        schemas::output_schema(Generation::Core, NAME)
    }

    fn permission(&self) -> Option<&str> {
        Some("edit")
    }

    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let input = edit_input(decode(&input, CORE_EDIT).map_err(core_failure)?);
        if input.old_string == input.new_string {
            return Err(ToolFailure::new(
                "No changes to apply: oldString and newString are identical.",
            ));
        }
        if input.old_string.is_empty() {
            return Err(ToolFailure::new(
                "oldString must not be empty. Use write to create or overwrite a file.",
            ));
        }
        let unable = || ToolFailure::new(format!("Unable to edit {}", input.path));
        let target = self
            .services
            .resolve(&input.path, ResolveKind::File)
            .await
            .map_err(|_| unable())?;
        self.services
            .authorize(&target, NAME, context, None)
            .await
            .map_err(|_| unable())?;
        let content = self
            .services
            .fs
            .read(&target.canonical)
            .await
            .map_err(|_| unable())?;
        let (bom, text) = decode_utf8(&content);
        let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let old = to_line_ending(&input.old_string, ending);
        let new = to_line_ending(&input.new_string, ending);
        let replacements = text.matches(old.as_str()).count();
        if replacements == 0 {
            return Err(ToolFailure::new(
                "Could not find oldString in the file. It must match exactly, including whitespace and indentation.",
            ));
        }
        let replace_all = input.replace_all == Some(true);
        if replacements > 1 && !replace_all {
            return Err(ToolFailure::new(
                "Found multiple exact matches for oldString. Provide more surrounding context or set replaceAll to true.",
            ));
        }
        let replaced = js_string_replace(&text, &old, &new, replace_all);
        let (additions, deletions) = line_counts(&text, &replaced);
        let (next_bom, next) = match replaced.strip_prefix('\u{FEFF}') {
            Some(rest) => (true, rest),
            None => (false, replaced.as_str()),
        };
        let output = if bom || next_bom {
            format!("\u{FEFF}{next}")
        } else {
            next.to_string()
        };
        let result = self
            .services
            .mutation
            .write_if_unchanged(&target, output.as_bytes(), &content)
            .await
            .map_err(|error| match error {
                MutationError::StaleContent { .. } => ToolFailure::new(STALE),
                _ => unable(),
            })?;
        let structured = json!({
            "files": [{
                "file": result.resource,
                "patch": create_two_files_patch(&result.resource, &result.resource, &text, &replaced),
                "additions": additions,
                "deletions": deletions,
                "status": "modified",
            }],
            "replacements": replacements,
        });
        let text = model_output(
            &result.resource,
            replacements,
            &input.old_string,
            &input.new_string,
        );
        Ok(text_output(structured, text))
    }
}

/// `toModelOutput`: header, replacement count and a bounded `-`/`+` preview of the strings.
pub fn model_output(file: &str, replacements: usize, old: &str, new: &str) -> String {
    let mut lines = vec![
        format!("Edited file successfully: {file}"),
        format!("Replacements: {replacements}"),
        "```diff".to_string(),
    ];
    lines.extend(preview(old, '-'));
    lines.extend(preview(new, '+'));
    lines.push("```".into());
    lines.join("\n")
}

fn preview(value: &str, prefix: char) -> Vec<String> {
    let normalized = value.replace("\r\n", "\n");
    let all: Vec<&str> = normalized.split('\n').collect();
    let mut shown: Vec<String> = all
        .iter()
        .take(6)
        .map(|line| match take_utf16(line, 240) {
            Some(head) => format!("{prefix}{head}..."),
            None => format!("{prefix}{line}"),
        })
        .collect();
    if all.len() > shown.len() {
        shown.push(format!("{prefix}..."));
    }
    shown
}

/// The first `count` UTF-16 units when the line is longer than that, `None` otherwise. A split
/// surrogate pair (a lone surrogate in JS) is not representable and is dropped.
fn take_utf16(line: &str, count: usize) -> Option<&str> {
    let mut units = 0;
    for (index, character) in line.char_indices() {
        if units + character.len_utf16() > count {
            return Some(&line[..index]);
        }
        units += character.len_utf16();
    }
    None
}

/// `decodeUtf8`: strip UTF-8 BOM bytes, then `TextDecoder` (lossy, and its default BOM handling
/// strips one more leading BOM).
fn decode_utf8(content: &[u8]) -> (bool, String) {
    let bom = content.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if bom { &content[3..] } else { content };
    let body = body.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(body);
    (bom, String::from_utf8_lossy(body).into_owned())
}

/// Core `convertToLineEnding`: normalize CRLF to LF, then expand when the file uses CRLF.
fn to_line_ending(text: &str, ending: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    if ending == "\n" {
        return normalized;
    }
    normalized.replace('\n', "\r\n")
}
