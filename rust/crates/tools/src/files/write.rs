//! Purpose: Core (V2) and legacy (V1) `write` tools. Core resolves through the Location, approves
//! external then edit, and commits through `FileMutation::write_text_preserving_bom`. Legacy
//! resolves lexically, approves external_directory then edit with a diff, writes, then runs the
//! formatter/event/LSP hooks.
//! Scope: `packages/core/src/tool/write.ts` and `packages/opencode/src/tool/write.ts` execute.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- tool-api/src/context.rs
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/output.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/jsdiff.rs
//! |-- tools/src/files/mutation_input.rs
//! |-- tools/src/files/patch.rs
//! |-- tools/src/files/replacers.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- tools/src/files/mod.rs
//! Invariants: core maps every failure after decoding to `Unable to write <path>` and reads no
//! content before approval, legacy reads the old content before approval only to build the diff
//! shown in the permission request (baseline order) and preserves an existing BOM.
//! Contract tests: tools/tests/files_write.rs

use super::common::{FileServices, Generation, legacy_output, text_output};
use super::edit_legacy::{EditHooks, Event, format_and_sync, read_bom, report_diagnostics};
use super::jsdiff::create_two_files_patch;
use super::mutation_input::{
    CORE_WRITE, LEGACY_WRITE, core_failure, decode, legacy_failure, write_input,
};
use super::patch::{join_bom, split_bom};
use super::replacers::trim_diff;
use super::schemas;
use async_trait::async_trait;
use opencode_platform_api::fs::{MutationOperation, ResolveKind};
use opencode_tool_api::{Tool, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::Arc;

pub const NAME: &str = "write";

#[derive(Debug, Clone)]
pub struct CoreWriteTool {
    services: FileServices,
}

impl CoreWriteTool {
    pub fn new(services: FileServices) -> Self {
        Self { services }
    }
}

#[async_trait]
impl Tool for CoreWriteTool {
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
        let (path, content) = write_input(decode(&input, CORE_WRITE).map_err(core_failure)?, false);
        let unable = || ToolFailure::new(format!("Unable to write {path}"));
        let target = self
            .services
            .resolve(&path, ResolveKind::File)
            .await
            .map_err(|_| unable())?;
        self.services
            .authorize(&target, "edit", context, None)
            .await
            .map_err(|_| unable())?;
        let result = self
            .services
            .mutation
            .write_text_preserving_bom(&target, &content)
            .await
            .map_err(|_| unable())?;
        let operation = match result.operation {
            MutationOperation::Write => "write",
            MutationOperation::Remove => "remove",
        };
        let text = format!(
            "{} file successfully: {}",
            if result.existed { "Wrote" } else { "Created" },
            result.resource
        );
        let structured = json!({
            "operation": operation,
            "target": result.target.to_string_lossy(),
            "resource": result.resource,
            "existed": result.existed,
        });
        Ok(text_output(structured, text))
    }
}

const MAX_PROJECT_DIAGNOSTICS_FILES: usize = 5;

#[derive(Debug, Clone)]
pub struct LegacyWriteTool {
    services: FileServices,
    hooks: Arc<dyn EditHooks>,
}

impl LegacyWriteTool {
    pub fn new(services: FileServices, hooks: Arc<dyn EditHooks>) -> Self {
        Self { services, hooks }
    }
}

#[async_trait]
impl Tool for LegacyWriteTool {
    fn description(&self) -> &str {
        schemas::description(Generation::Legacy, NAME)
    }

    fn input_schema(&self) -> Value {
        schemas::input_schema(Generation::Legacy, NAME)
    }

    fn permission(&self) -> Option<&str> {
        Some("edit")
    }

    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let (path, content) = write_input(
            decode(&input, LEGACY_WRITE).map_err(|detail| legacy_failure(NAME, detail))?,
            true,
        );
        let fail = |error: String| ToolFailure::new(error);
        let filepath = if Path::new(&path).is_absolute() {
            std::path::PathBuf::from(&path)
        } else {
            self.services.legacy_path(&path)
        };
        let shown = filepath.to_string_lossy().into_owned();
        self.services
            .authorize_legacy_external(&filepath, ResolveKind::File, false, context)
            .await?;
        let fs = self.services.fs.as_ref();
        let exists = fs.exists(&filepath).await.unwrap_or(false);
        let (bom, old) = if exists {
            read_bom(fs, &filepath).await.map_err(fail)?
        } else {
            (false, String::new())
        };
        let (next_bom, next) = split_bom(&content);
        let desired = bom || next_bom;
        let diff = trim_diff(&create_two_files_patch(&shown, &shown, &old, next));
        let title = self.services.title(&filepath);
        let mut metadata = Map::new();
        metadata.insert("filepath".into(), json!(shown));
        metadata.insert("diff".into(), json!(diff));
        context
            .authorize(
                "edit",
                vec![title.clone()],
                Some(vec!["*".into()]),
                Some(metadata),
            )
            .await?;
        fs.write_with_dirs(&filepath, join_bom(next, desired).as_bytes())
            .await
            .map_err(|error| fail(error.to_string()))?;
        format_and_sync(self.hooks.as_ref(), fs, &filepath, desired)
            .await
            .map_err(fail)?;
        self.hooks.publish(Event::Edited {
            file: shown.clone(),
        });
        self.hooks.publish(Event::Updated {
            file: shown.clone(),
            event: if exists { "change" } else { "add" },
        });
        let mut output = "Wrote file successfully.".to_string();
        self.hooks.touch(&filepath).await;
        let diagnostics = self.hooks.diagnostics().await;
        let mut project = 0;
        for (file, issues) in diagnostics.iter() {
            let current = *file == shown;
            if !current && project >= MAX_PROJECT_DIAGNOSTICS_FILES {
                continue;
            }
            let block = report_diagnostics(if current { &shown } else { file }, issues);
            if block.is_empty() {
                continue;
            }
            if current {
                output.push_str(&format!(
                    "\n\nLSP errors detected in this file, please fix:\n{block}"
                ));
                continue;
            }
            project += 1;
            output.push_str(&format!("\n\nLSP errors detected in other files:\n{block}"));
        }
        let metadata = json!({
            "diagnostics": diagnostics,
            "filepath": shown,
            "exists": exists,
        });
        Ok(legacy_output(title, metadata, output))
    }
}
