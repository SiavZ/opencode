//! Purpose: Legacy (V1) `edit` tool: per-path serialized fuzzy replacement through the replacer
//! chain, empty-oldString file creation, diff-bearing edit approval, formatter BOM resync, file
//! events and LSP diagnostics, plus the hook port and helpers shared with legacy `write`.
//! Scope: `packages/opencode/src/tool/edit.ts` execute (replacers live in replacers.rs).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/jsdiff.rs
//! |-- tools/src/files/mutation_input.rs
//! |-- tools/src/files/patch.rs
//! |-- tools/src/files/replacers.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! |-- async_trait::async_trait
//! |-- schemas::description
//! |-- schemas::input_schema
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::sync::Mutex
//! Used by:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/write.rs
//! |-- tools/tests/files_mutation/legacy_edit.rs
//! |-- tools/tests/files_mutation/legacy_edit_lines.rs
//! |-- tools/tests/files_mutation/native.rs
//! |-- tools/tests/files_mutation/support.rs
//! `-- tools/tests/files_mutation/write.rs
//! Invariants: external_directory approval precedes the lock, the lock (keyed by the resolved
//! real path) spans read, approval and write so concurrent edits of one file never lose updates,
//! nothing is written when approval fails, diffs are LF-normalized and BOM-free.
//! Contract tests: tools/tests/files_edit.rs

use super::common::{FileServices, Generation, legacy_output};
use super::jsdiff::{create_two_files_patch, line_counts};
use super::mutation_input::{LEGACY_EDIT, decode, edit_input, legacy_failure};
use super::patch::{join_bom, split_bom};
use super::replacers::{replace, trim_diff};
use super::schemas;
use async_trait::async_trait;
use opencode_platform_api::fs::{FileSystem, FileType, ResolveKind};
use opencode_tool_api::{Tool, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const NAME: &str = "edit";
const MAX_DIAGNOSTICS_PER_FILE: usize = 20;

/// File events published after a legacy write (`FileSystem.Event.Edited`, `Watcher.Event.Updated`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Edited { file: String },
    Updated { file: String, event: &'static str },
}

/// Session/instance collaborators of the legacy mutation tools: `Format.file`, the event bridge,
/// and the LSP service (`touchFile(file, "document")` then `diagnostics()`).
#[async_trait]
pub trait EditHooks: Send + Sync + std::fmt::Debug {
    /// Run configured formatters, `true` when one ran.
    async fn format(&self, file: &Path) -> bool;
    fn publish(&self, event: Event);
    async fn touch(&self, file: &Path);
    /// All known diagnostics keyed by normalized path, values are arrays of LSP diagnostics.
    async fn diagnostics(&self) -> Map<String, Value>;
}

/// No formatter, no subscribers and no LSP servers configured.
#[derive(Debug, Default)]
pub struct NoEditHooks;

#[async_trait]
impl EditHooks for NoEditHooks {
    async fn format(&self, _file: &Path) -> bool {
        false
    }
    fn publish(&self, _event: Event) {}
    async fn touch(&self, _file: &Path) {}
    async fn diagnostics(&self) -> Map<String, Value> {
        Map::new()
    }
}

#[derive(Debug, Clone)]
pub struct LegacyEditTool {
    services: FileServices,
    hooks: Arc<dyn EditHooks>,
}

impl LegacyEditTool {
    pub fn new(services: FileServices, hooks: Arc<dyn EditHooks>) -> Self {
        Self { services, hooks }
    }
}

struct Applied {
    old: String,
    new: String,
    diff: String,
}

#[async_trait]
impl Tool for LegacyEditTool {
    fn description(&self) -> &str {
        schemas::description(Generation::Legacy, NAME)
    }

    fn input_schema(&self) -> Value {
        schemas::input_schema(Generation::Legacy, NAME)
    }

    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let input =
            edit_input(decode(&input, LEGACY_EDIT).map_err(|detail| legacy_failure(NAME, detail))?);
        if input.path.is_empty() {
            return Err(ToolFailure::new("filePath is required"));
        }
        if input.old_string == input.new_string {
            return Err(ToolFailure::new(
                "No changes to apply: oldString and newString are identical.",
            ));
        }
        let filepath = if Path::new(&input.path).is_absolute() {
            PathBuf::from(&input.path)
        } else {
            self.services.legacy_path(&input.path)
        };
        let shown = filepath.to_string_lossy().into_owned();
        self.services
            .authorize_legacy_external(&filepath, ResolveKind::File, false, context)
            .await?;
        let lock = lock(self.services.fs.as_ref(), &filepath).await;
        let applied = {
            let _guard = lock.lock().await;
            self.apply(&filepath, &shown, &input, context).await?
        };
        let (additions, deletions) = line_counts(&applied.old, &applied.new);
        let filediff = json!({"file": shown, "patch": applied.diff, "additions": additions, "deletions": deletions});
        let mut progress = Map::new();
        progress.insert("diff".into(), json!(applied.diff));
        progress.insert("filediff".into(), filediff.clone());
        progress.insert("diagnostics".into(), json!({}));
        context.progress.update(None, &progress);
        let mut output = "Edit applied successfully.".to_string();
        self.hooks.touch(&filepath).await;
        let diagnostics = self.hooks.diagnostics().await;
        if let Some(issues) = diagnostics.get(&shown) {
            let block = report_diagnostics(&shown, issues);
            if !block.is_empty() {
                output.push_str(&format!(
                    "\n\nLSP errors detected in this file, please fix:\n{block}"
                ));
            }
        }
        let metadata =
            json!({"diagnostics": diagnostics, "diff": applied.diff, "filediff": filediff});
        Ok(legacy_output(
            self.services.title(&filepath),
            metadata,
            output,
        ))
    }
}

impl LegacyEditTool {
    async fn apply(
        &self,
        filepath: &Path,
        shown: &str,
        input: &super::mutation_input::EditInput,
        context: &ToolContext,
    ) -> Result<Applied, ToolFailure> {
        let fs = self.services.fs.as_ref();
        let fail = |message: String| ToolFailure::new(message);
        if input.old_string.is_empty() {
            if fs.exists(filepath).await.unwrap_or(false) {
                return Err(ToolFailure::new(
                    "oldString cannot be empty when editing an existing file. Provide the exact text to replace, or use write for an intentional full-file replacement.",
                ));
            }
            let (bom, text) = split_bom(&input.new_string);
            let mut new = text.to_string();
            let diff = trim_diff(&create_two_files_patch(shown, shown, "", &new));
            self.ask(filepath, shown, &diff, context).await?;
            fs.write_with_dirs(filepath, join_bom(&new, bom).as_bytes())
                .await
                .map_err(|error| fail(error.to_string()))?;
            if let Some(synced) = format_and_sync(self.hooks.as_ref(), fs, filepath, bom)
                .await
                .map_err(fail)?
            {
                new = synced;
            }
            self.publish(shown, "add");
            return Ok(Applied {
                old: String::new(),
                new,
                diff,
            });
        }
        let info = fs.metadata(filepath).await.ok();
        let Some(info) = info else {
            return Err(fail(format!("File {shown} not found")));
        };
        if info.file_type == FileType::Directory {
            return Err(fail(format!("Path is a directory, not a file: {shown}")));
        }
        let (bom, old) = read_bom(fs, filepath).await.map_err(fail)?;
        let crlf = old.contains("\r\n");
        let convert = |text: &str| {
            let normalized = text.replace("\r\n", "\n");
            if crlf {
                normalized.replace('\n', "\r\n")
            } else {
                normalized
            }
        };
        let replaced = replace(
            &old,
            &convert(&input.old_string),
            &convert(&input.new_string),
            input.replace_all == Some(true),
        )?;
        let (next_bom, next) = split_bom(&replaced);
        let desired = bom || next_bom;
        let mut new = next.to_string();
        let lf = |text: &str| text.replace("\r\n", "\n");
        let diff = trim_diff(&create_two_files_patch(shown, shown, &lf(&old), &lf(&new)));
        self.ask(filepath, shown, &diff, context).await?;
        fs.write_with_dirs(filepath, join_bom(&new, desired).as_bytes())
            .await
            .map_err(|error| fail(error.to_string()))?;
        if let Some(synced) = format_and_sync(self.hooks.as_ref(), fs, filepath, desired)
            .await
            .map_err(fail)?
        {
            new = synced;
        }
        self.publish(shown, "change");
        let diff = trim_diff(&create_two_files_patch(shown, shown, &lf(&old), &lf(&new)));
        Ok(Applied { old, new, diff })
    }

    async fn ask(
        &self,
        filepath: &Path,
        shown: &str,
        diff: &str,
        context: &ToolContext,
    ) -> Result<(), ToolFailure> {
        let mut metadata = Map::new();
        metadata.insert("filepath".into(), json!(shown));
        metadata.insert("diff".into(), json!(diff));
        context
            .authorize(
                "edit",
                vec![self.services.title(filepath)],
                Some(vec!["*".into()]),
                Some(metadata),
            )
            .await?;
        Ok(())
    }

    fn publish(&self, shown: &str, event: &'static str) {
        self.hooks.publish(Event::Edited { file: shown.into() });
        self.hooks.publish(Event::Updated {
            file: shown.into(),
            event,
        });
    }
}

/// Process-wide per-file locks keyed by `FSUtil.resolve` (real path, or the lexical path when
/// missing), as the module-level `locks` map in the baseline.
async fn lock(fs: &dyn FileSystem, filepath: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = fs
        .real_path(filepath)
        .await
        .unwrap_or_else(|_| filepath.to_path_buf());
    LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(key)
        .or_default()
        .clone()
}

/// `Bom.readFile`: lossy UTF-8 decode keeping a BOM (`ignoreBOM: true`), then split one BOM.
pub async fn read_bom(fs: &dyn FileSystem, filepath: &Path) -> Result<(bool, String), String> {
    let bytes = fs.read(filepath).await.map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let (bom, text) = split_bom(&text);
    Ok((bom, text.to_string()))
}

/// `if (yield* format.file(path)) Bom.syncFile(...)`: restore the desired BOM after a formatter
/// ran and return the formatted text, `None` when no formatter ran.
pub async fn format_and_sync(
    hooks: &dyn EditHooks,
    fs: &dyn FileSystem,
    filepath: &Path,
    bom: bool,
) -> Result<Option<String>, String> {
    if !hooks.format(filepath).await {
        return Ok(None);
    }
    let (current, text) = read_bom(fs, filepath).await?;
    if current != bom {
        fs.write_with_dirs(filepath, join_bom(&text, bom).as_bytes())
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(Some(text))
}

/// `LSP.Diagnostic.report`: severity-1 issues only, at most 20, empty string when none.
pub fn report_diagnostics(file: &str, issues: &Value) -> String {
    let errors: Vec<&Value> = issues
        .as_array()
        .map(|items| items.iter().filter(|item| item["severity"] == 1).collect())
        .unwrap_or_default();
    if errors.is_empty() {
        return String::new();
    }
    let body = errors
        .iter()
        .take(MAX_DIAGNOSTICS_PER_FILE)
        .map(|item| {
            let start = &item["range"]["start"];
            let message = item["message"].as_str().unwrap_or_default();
            format!(
                "ERROR [{}:{}] {message}",
                start["line"].as_i64().unwrap_or(0) + 1,
                start["character"].as_i64().unwrap_or(0) + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let more = errors.len().saturating_sub(MAX_DIAGNOSTICS_PER_FILE);
    let suffix = if more > 0 {
        format!("\n... and {more} more")
    } else {
        String::new()
    };
    format!("<diagnostics file=\"{file}\">\n{body}{suffix}\n</diagnostics>")
}
