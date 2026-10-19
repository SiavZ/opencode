//! Purpose: compose native file and shell tools with configured authorization and retained output.
//! Scope: Location-scoped tool registration and permission evaluation.
//! Uses:
//! rust/crates/
//! |-- config/src/lib.rs
//! |-- permissions/src/lib.rs
//! |-- platform-native/src/fs/mod.rs
//! |-- platform-native/src/process/mod.rs
//! |-- runtime/src/location_agents.rs
//! |-- schema/src/permission.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/write.rs
//! `-- tools/src/shell/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: authorization precedes native access; oversized text is retained under process data.
//! Contract tests: runtime/tests/production.rs

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use opencode_config::CurrentConfig;
use opencode_permissions::PermissionService;
use opencode_platform_native::fs::{NativeFileMutation, NativeFileSystem, NativeLocationMutation};
use opencode_platform_native::process::NativeProcessRunner;
use opencode_session_runtime::runner::ports::RegistryMaterializer;
use opencode_tool_api::{
    Authorizer, BoundOutput, OutputBounder, Tool, ToolContent, ToolOutput, ToolRegistry,
};
use opencode_tools::files::{
    FileServices,
    edit::CoreEditTool,
    read_core::{CoreReadTool, NoResizer},
    write::CoreWriteTool,
};
use opencode_tools::interaction::{QuestionTool, Questions};
use opencode_tools::search::{
    CoreGlobTool, CoreGrepTool, NoInstaller, Ripgrep, RipgrepBinary, SearchServices,
};
use opencode_tools::shell::{CoreBashTool, DirectoryRetention, OutputRetention};

#[path = "location_legacy_tools.rs"]
mod legacy;

pub struct Tools {
    pub legacy_materializer: RegistryMaterializer,
    pub legacy_registration: opencode_tool_api::Registration,
    pub materializer: RegistryMaterializer,
    pub registration: opencode_tool_api::Registration,
}

pub async fn build_with_permissions(
    directory: &Path,
    project: &Path,
    data: &Path,
    config: &CurrentConfig,
    permissions: Arc<PermissionService>,
    application_tools: &ToolRegistry,
    questions: Arc<Questions>,
) -> Result<Tools, String> {
    build_authorized(
        directory,
        project,
        data,
        config,
        permissions,
        application_tools,
        questions,
    )
    .await
}

async fn build_authorized(
    directory: &Path,
    project: &Path,
    data: &Path,
    config: &CurrentConfig,
    authorizer: Arc<dyn Authorizer>,
    application_tools: &ToolRegistry,
    questions: Arc<Questions>,
) -> Result<Tools, String> {
    let location = Arc::new(
        NativeLocationMutation::new(directory)
            .await
            .map_err(|e| e.to_string())?,
    );
    let files = FileServices::new(
        Arc::new(NativeFileSystem),
        Arc::new(NativeFileMutation::new()),
        location.clone(),
        directory.to_owned(),
        project.to_owned(),
    );
    let registry = application_tools.fork(Arc::new(RetainedOutput(DirectoryRetention::new(
        data.join("tool-output"),
    ))));
    let search = SearchServices {
        ripgrep: Ripgrep::new(
            Arc::new(NativeProcessRunner::new()),
            Arc::new(RipgrepBinary::new(
                data.join("bin"),
                None,
                Arc::new(NoInstaller),
            )),
        ),
        directory: directory.to_owned(),
        worktree: project.to_owned(),
    };
    let (legacy_materializer, legacy_registration) = legacy::build(
        &files,
        &search,
        data,
        config,
        authorizer.clone(),
        questions.clone(),
        application_tools,
    )?;
    let tools: Vec<(String, Arc<dyn Tool>)> = vec![
        ("question".into(), Arc::new(QuestionTool(questions))),
        (
            "glob".into(),
            Arc::new(SecureSearch {
                tool: Arc::new(CoreGlobTool::new(search.clone())),
                files: files.clone(),
            }),
        ),
        (
            "grep".into(),
            Arc::new(SecureSearch {
                tool: Arc::new(CoreGrepTool::new(search)),
                files: files.clone(),
            }),
        ),
        (
            "read".into(),
            Arc::new(CoreReadTool::new(files.clone(), Arc::new(NoResizer))),
        ),
        ("write".into(), Arc::new(CoreWriteTool::new(files.clone()))),
        ("edit".into(), Arc::new(CoreEditTool::new(files))),
        (
            "bash".into(),
            Arc::new(CoreBashTool::new(
                Arc::new(NativeProcessRunner::new()),
                location,
                config
                    .latest("shell")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            )),
        ),
    ];
    let registration = registry.register(tools).map_err(|e| e.to_string())?;
    Ok(Tools {
        legacy_materializer,
        legacy_registration,
        materializer: RegistryMaterializer {
            registry,
            authorizer,
        },
        registration,
    })
}

struct SecureSearch {
    tool: Arc<dyn Tool>,
    files: FileServices,
}

#[async_trait]
impl Tool for SecureSearch {
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn input_schema(&self) -> serde_json::Value {
        self.tool.input_schema()
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        self.tool.output_schema()
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &opencode_tool_api::ToolContext,
    ) -> Result<ToolOutput, opencode_tool_api::ToolFailure> {
        // Reuse canonical filesystem policy before ripgrep observes any content.
        let target = self
            .files
            .resolve(
                input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("."),
                opencode_platform_api::fs::ResolveKind::Directory,
            )
            .await?;
        self.files.authorize_external(&target, context).await?;
        let mut input = input;
        if let Some(object) = input.as_object_mut() {
            object.insert(
                "path".into(),
                serde_json::Value::String(target.canonical.display().to_string()),
            );
        }
        self.tool.execute(input, context).await
    }
}

struct RetainedOutput(DirectoryRetention);

#[async_trait]
impl OutputBounder for RetainedOutput {
    async fn bound(&self, _: &str, _: &str, mut output: ToolOutput) -> Result<BoundOutput, String> {
        let mut paths = Vec::new();
        for content in &mut output.content {
            if let ToolContent::Text { text } = content {
                if text.len() <= 50 * 1024 && text.lines().count() <= 2000 {
                    continue;
                }
                let path = self.0.write(text).await?;
                let bounded = text.lines().take(2000).collect::<Vec<_>>().join("\n");
                let end = bounded
                    .char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(bounded.len()))
                    .take_while(|i| *i <= 50 * 1024)
                    .last()
                    .unwrap_or(0);
                *text = format!(
                    "{}\n\nOutput truncated. Full output: {}",
                    &bounded[..end],
                    path.display()
                );
                paths.push(path.display().to_string());
            }
        }
        Ok(BoundOutput {
            output,
            output_paths: paths,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn retention_keeps_last_character_and_full_native_file() {
        let root = tempfile::tempdir().unwrap();
        let full = format!("{}é", "a\n".repeat(2000));
        let bounder = RetainedOutput(DirectoryRetention::new(root.path()));
        let bounded = bounder
            .bound(
                "session",
                "call",
                ToolOutput::new(json!({}), vec![ToolContent::text(&full)]),
            )
            .await
            .unwrap();
        let ToolContent::Text { text } = &bounded.output.content[0] else {
            panic!("text output")
        };
        assert!(text.starts_with(&"a\n".repeat(1999)));
        assert!(text.contains("a\n\nOutput truncated."));
        assert_eq!(
            tokio::fs::read_to_string(&bounded.output_paths[0])
                .await
                .unwrap(),
            full
        );
        let full = format!("{}é", "a".repeat(50 * 1024 - 1));
        let bounded = bounder
            .bound(
                "session",
                "call",
                ToolOutput::new(json!({}), vec![ToolContent::text(&full)]),
            )
            .await
            .unwrap();
        let ToolContent::Text { text } = &bounded.output.content[0] else {
            panic!("text output")
        };
        assert!(text.starts_with(&"a".repeat(50 * 1024 - 1)));
        assert!(!text.contains('�'));
    }
}
