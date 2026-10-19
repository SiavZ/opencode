//! Purpose: compose generation-correct legacy native tools using shared service instances.
//! Scope: V1 schemas and native implementations, not argument translation.
//! Uses: runtime/location_tools; tool-api; tools/files/search/shell/interaction.
//! Used by: runtime/location_tools and legacy child/provider materializers.
//! Invariants: V1 and V2 registries share application registrations and the same permission domain.
//! Contract tests: runtime legacy provider acceptance and tools generation suites.

use super::RetainedOutput;
use opencode_config::CurrentConfig;
use opencode_platform_native::process::NativeProcessRunner;
use opencode_session_runtime::runner::ports::RegistryMaterializer;
use opencode_tool_api::{Authorizer, Registration, Tool, ToolRegistry};
use opencode_tools::files::{
    FileServices,
    edit_legacy::{LegacyEditTool, NoEditHooks},
    read_legacy::{LegacyReadTool, NoHooks},
    write::LegacyWriteTool,
};
use opencode_tools::interaction::{LegacyQuestionTool, Questions};
use opencode_tools::search::{LegacyGlobTool, LegacyGrepTool, SearchServices};
use opencode_tools::shell::{DirectoryRetention, LegacyShellConfig, LegacyShellTool, NoShellEnv};
use std::path::Path;
use std::sync::Arc;

pub fn build(
    files: &FileServices,
    search: &SearchServices,
    data: &Path,
    config: &CurrentConfig,
    authorizer: Arc<dyn Authorizer>,
    questions: Arc<Questions>,
    application_tools: &ToolRegistry,
) -> Result<(RegistryMaterializer, Registration), String> {
    let registry = application_tools.fork(Arc::new(RetainedOutput(DirectoryRetention::new(
        data.join("tool-output"),
    ))));
    let tools: Vec<(String, Arc<dyn Tool>)> = vec![
        (
            "read".into(),
            Arc::new(LegacyReadTool::new(files.clone(), Arc::new(NoHooks))),
        ),
        (
            "write".into(),
            Arc::new(LegacyWriteTool::new(files.clone(), Arc::new(NoEditHooks))),
        ),
        (
            "edit".into(),
            Arc::new(LegacyEditTool::new(files.clone(), Arc::new(NoEditHooks))),
        ),
        ("glob".into(), Arc::new(LegacyGlobTool::new(search.clone()))),
        ("grep".into(), Arc::new(LegacyGrepTool::new(search.clone()))),
        ("question".into(), Arc::new(LegacyQuestionTool(questions))),
        (
            "bash".into(),
            Arc::new(LegacyShellTool::new(
                Arc::new(NativeProcessRunner::new()),
                Arc::new(DirectoryRetention::new(data.join("tool-output"))),
                Arc::new(NoShellEnv),
                LegacyShellConfig {
                    shell: config
                        .latest("shell")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    default_timeout_ms: 120_000,
                    tmp: std::env::temp_dir().display().to_string(),
                    home: std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| data.to_owned()),
                    directory: files.directory.clone(),
                    worktree: files.worktree.clone(),
                },
            )),
        ),
    ];
    let registration = registry
        .register(tools)
        .map_err(|error| error.to_string())?;
    Ok((
        RegistryMaterializer {
            registry,
            authorizer,
        },
        registration,
    ))
}
