//! Purpose: Legacy (V1) `read` tool: numbered line windows, directory listings, image/PDF
//! attachments, nearby instruction reminders and the legacy approval order.
//! Scope: `packages/opencode/src/tool/read.ts` execute over the platform `FileSystem` port.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/lib.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/read_common.rs
//! |-- tools/src/files/read_input.rs
//! |-- tools/src/files/read_lines.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! |-- async_trait::async_trait
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- schemas::description
//! |-- schemas::input_schema
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! `-- tools/tests/files_read/legacy.rs
//! Invariants: the stat probe is metadata only, external_directory then read approval precede
//! every content byte, the output is never passed through generic truncation.
//! Contract tests: tools/tests/files_read.rs

use super::common::{FileServices, Generation, legacy_output};
use super::read_common::{
    SUPPORTED_IMAGE_MIMES, is_binary, locale_compare, mime_type, sniff_attachment_mime,
};
use super::read_input::decode_legacy;
use super::read_lines::{Window, lines};
use super::schemas;
use async_trait::async_trait;
use base64::Engine;
use opencode_platform_api::fs::{FileType, ResolveKind};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub const NAME: &str = "read";
const DEFAULT_READ_LIMIT: u64 = 2_000;
const SAMPLE_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedInstruction {
    pub filepath: String,
    /// Already prefixed `Instructions from: <path>\n`.
    pub content: String,
}

/// Session-owned collaborators of the legacy tool: `Instruction.resolve` (per-message claims and
/// already-loaded instructions) and fire-and-forget LSP warm-up (`lsp.touchFile`).
#[async_trait]
pub trait LegacyReadHooks: Send + Sync + std::fmt::Debug {
    async fn instructions(
        &self,
        filepath: &Path,
        context: &ToolContext,
    ) -> Result<Vec<LoadedInstruction>, String>;
    fn warm(&self, filepath: &Path);
}

/// No instruction files and no LSP, as when both services have nothing configured.
#[derive(Debug, Default)]
pub struct NoHooks;

#[async_trait]
impl LegacyReadHooks for NoHooks {
    async fn instructions(
        &self,
        _filepath: &Path,
        _context: &ToolContext,
    ) -> Result<Vec<LoadedInstruction>, String> {
        Ok(vec![])
    }
    fn warm(&self, _filepath: &Path) {}
}

#[derive(Debug, Clone)]
pub struct LegacyReadTool {
    services: FileServices,
    hooks: Arc<dyn LegacyReadHooks>,
}

impl LegacyReadTool {
    pub fn new(services: FileServices, hooks: Arc<dyn LegacyReadHooks>) -> Self {
        Self { services, hooks }
    }

    /// Execute with the per-call `ctx.extra.bypassCwdCheck` flag (user-attached files).
    pub async fn read(
        &self,
        input: Value,
        context: &ToolContext,
        bypass: bool,
    ) -> Result<ToolOutput, ToolFailure> {
        let input = decode_legacy(&input).map_err(|detail| {
            ToolFailure::new(format!(
                "The {NAME} tool was called with invalid arguments: SchemaError({detail}).\nPlease rewrite the input so it satisfies the expected schema."
            ))
        })?;
        let filepath = if Path::new(&input.path).is_absolute() {
            PathBuf::from(&input.path)
        } else {
            self.services.legacy_path(&input.path)
        };
        let title = self.services.title(&filepath);
        let stat = match self.services.fs.metadata(&filepath).await {
            Ok(stat) => Some(stat),
            Err(error) if error.kind == PlatformErrorKind::NotFound => None,
            Err(error) => return Err(platform(error)),
        };
        let is_directory = stat.as_ref().map(|stat| stat.file_type) == Some(FileType::Directory);
        let kind = if is_directory {
            ResolveKind::Directory
        } else {
            ResolveKind::File
        };
        self.services
            .authorize_legacy_external(&filepath, kind, bypass, context)
            .await?;
        context
            .authorize(
                NAME,
                vec![title.clone()],
                Some(vec!["*".into()]),
                Some(serde_json::Map::new()),
            )
            .await?;
        let Some(stat) = stat else {
            return Err(self.miss(&filepath).await);
        };
        let shown = filepath.to_string_lossy().into_owned();
        let window = Window {
            offset: input.offset.filter(|offset| *offset > 0).unwrap_or(1),
            limit: input.limit.unwrap_or(DEFAULT_READ_LIMIT),
        };
        if is_directory {
            return self.directory(&filepath, title, window).await;
        }
        let loaded = self
            .hooks
            .instructions(&filepath, context)
            .await
            .map_err(ToolFailure::new)?;
        let loaded_paths: Vec<&str> = loaded.iter().map(|item| item.filepath.as_str()).collect();
        let sample = if stat.size == 0 {
            vec![]
        } else {
            self.services
                .fs
                .read_range(&filepath, 0, SAMPLE_BYTES.min(stat.size as usize))
                .await
                .map_err(platform)?
        };
        let mime = sniff_attachment_mime(&sample, mime_type(&shown));
        if SUPPORTED_IMAGE_MIMES.contains(&mime.as_str()) || mime == "application/pdf" {
            let bytes = self.services.fs.read(&filepath).await.map_err(platform)?;
            self.stamp(context, &filepath, stat.modified);
            let message = if mime == "application/pdf" {
                "PDF read successfully"
            } else {
                "Image read successfully"
            };
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let metadata = json!({"preview": message, "truncated": false, "loaded": loaded_paths});
            let mut output = legacy_output(title, metadata, message.into());
            let url = format!("data:{mime};base64,{data}");
            output.structured["attachments"] = json!([{"type": "file", "mime": mime, "url": url}]);
            output
                .content
                .push(ToolContent::file_base64(&data, &mime, None));
            return Ok(output);
        }
        if is_binary(&shown, &sample) {
            return Err(ToolFailure::new(format!(
                "Cannot read binary file: {shown}"
            )));
        }
        let file = lines(self.services.fs.as_ref(), &filepath, window)
            .await
            .map_err(platform)?;
        if file.count < window.offset && !(file.count == 0 && window.offset == 1) {
            return Err(ToolFailure::new(format!(
                "Offset {} is out of range for this file ({} lines)",
                window.offset, file.count
            )));
        }
        self.stamp(context, &filepath, stat.modified);
        let numbered: Vec<String> = file
            .raw
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{}: {line}", index as u64 + window.offset))
            .collect();
        let mut output = format!("<path>{shown}</path>\n<type>file</type>\n<content>\n");
        output.push_str(&numbered.join("\n"));
        let last = window.offset + file.raw.len() as u64 - 1;
        let next = last + 1;
        let truncated = file.more || file.cut;
        if file.cut {
            output.push_str(&format!(
                "\n\n(Output capped at 50 KB. Showing lines {}-{last}. Use offset={next} to continue.)",
                window.offset
            ));
        } else if file.more {
            output.push_str(&format!(
                "\n\n(Showing lines {}-{last} of {}. Use offset={next} to continue.)",
                window.offset, file.count
            ));
        } else {
            output.push_str(&format!("\n\n(End of file - total {} lines)", file.count));
        }
        output.push_str("\n</content>");
        self.hooks.warm(&filepath);
        if !loaded.is_empty() {
            let reminders: Vec<&str> = loaded.iter().map(|item| item.content.as_str()).collect();
            output.push_str(&format!(
                "\n\n<system-reminder>\n{}\n</system-reminder>",
                reminders.join("\n\n")
            ));
        }
        let text = file.raw.join("\n");
        let metadata = json!({
            "preview": file.raw.iter().take(20).cloned().collect::<Vec<_>>().join("\n"),
            "truncated": truncated,
            "loaded": loaded_paths,
            "display": {
                "type": "file",
                "path": shown,
                "text": text,
                "lineStart": window.offset,
                "lineEnd": last,
                "totalLines": file.count,
                "truncated": truncated,
            },
        });
        Ok(legacy_output(title, metadata, output))
    }

    async fn directory(
        &self,
        filepath: &Path,
        title: String,
        window: Window,
    ) -> Result<ToolOutput, ToolFailure> {
        let fs = self.services.fs.as_ref();
        let mut items = Vec::new();
        for (name, kind) in fs.read_dir(filepath).await.map_err(platform)? {
            let directory = match kind {
                FileType::Directory => true,
                FileType::SymbolicLink => fs
                    .metadata(&filepath.join(&name))
                    .await
                    .is_ok_and(|info| info.file_type == FileType::Directory),
                _ => false,
            };
            items.push(if directory { format!("{name}/") } else { name });
        }
        items.sort_by(|a, b| locale_compare(a, b));
        let start = (window.offset - 1) as usize;
        let sliced: Vec<String> = items
            .iter()
            .skip(start)
            .take(window.limit as usize)
            .cloned()
            .collect();
        let truncated = start + sliced.len() < items.len();
        let shown = filepath.to_string_lossy().into_owned();
        let footer = if truncated {
            format!(
                "\n(Showing {} of {} entries. Use 'offset' parameter to read beyond entry {})",
                sliced.len(),
                items.len(),
                window.offset + sliced.len() as u64
            )
        } else {
            format!("\n({} entries)", items.len())
        };
        let output = [
            format!("<path>{shown}</path>"),
            "<type>directory</type>".into(),
            "<entries>".into(),
            sliced.join("\n"),
            footer,
            "</entries>".into(),
        ]
        .join("\n");
        let metadata = json!({
            "preview": sliced.iter().take(20).cloned().collect::<Vec<_>>().join("\n"),
            "truncated": truncated,
            "loaded": [],
            "display": {
                "type": "directory",
                "path": shown,
                "entries": sliced,
                "offset": window.offset,
                "totalEntries": items.len(),
                "truncated": truncated,
            },
        });
        Ok(legacy_output(title, metadata, output))
    }

    async fn miss(&self, filepath: &Path) -> ToolFailure {
        let directory = filepath.parent().unwrap_or(filepath);
        let base = filepath
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let mut names: Vec<String> = self
            .services
            .fs
            .read_dir(directory)
            .await
            .map(|items| items.into_iter().map(|(name, _)| name).collect())
            .unwrap_or_default();
        names.sort();
        let suggestions: Vec<String> = names
            .into_iter()
            .filter(|item| {
                let item = item.to_lowercase();
                item.contains(&base) || base.contains(&item)
            })
            .map(|item| directory.join(item).to_string_lossy().into_owned())
            .take(3)
            .collect();
        let shown = filepath.to_string_lossy();
        if suggestions.is_empty() {
            return ToolFailure::new(format!("File not found: {shown}"));
        }
        ToolFailure::new(format!(
            "File not found: {shown}\n\nDid you mean one of these?\n{}",
            suggestions.join("\n")
        ))
    }

    fn stamp(&self, context: &ToolContext, filepath: &Path, modified: Option<SystemTime>) {
        self.services.record_read(
            &context.session_id,
            filepath,
            modified.unwrap_or_else(SystemTime::now),
        );
    }
}

#[async_trait]
impl Tool for LegacyReadTool {
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
        self.read(input, context, false).await
    }
}

fn platform(error: PlatformError) -> ToolFailure {
    ToolFailure::new(error.to_string())
}
