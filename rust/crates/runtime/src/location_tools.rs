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
use opencode_permissions::{AssertInput, Effect, PermissionError, Rule, evaluate};
use opencode_platform_native::fs::{NativeFileMutation, NativeFileSystem, NativeLocationMutation};
use opencode_platform_native::process::NativeProcessRunner;
use opencode_session_runtime::agents::Agents;
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
use opencode_tools::shell::{CoreBashTool, DirectoryRetention, OutputRetention};

pub struct Tools {
    pub materializer: RegistryMaterializer,
    pub registration: opencode_tool_api::Registration,
}

pub async fn build(
    directory: &Path,
    project: &Path,
    data: &Path,
    config: &CurrentConfig,
    agents: Agents,
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
    let registry = ToolRegistry::new(Arc::new(RetainedOutput(DirectoryRetention::new(
        data.join("tool-output"),
    ))));
    let tools: Vec<(String, Arc<dyn Tool>)> = vec![
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
        materializer: RegistryMaterializer {
            registry,
            authorizer: Arc::new(ConfiguredAuthorization { agents }),
        },
        registration,
    })
}

struct ConfiguredAuthorization {
    agents: Agents,
}

#[async_trait]
impl Authorizer for ConfiguredAuthorization {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let selected = self.agents.select(input.agent.as_deref());
        let rules: Vec<Rule> = selected
            .info
            .map(|agent| {
                agent
                    .permissions
                    .into_iter()
                    .map(|rule| {
                        Rule::new(
                            rule.action,
                            rule.resource,
                            match rule.effect {
                                opencode_schema::permission::Effect::Allow => Effect::Allow,
                                opencode_schema::permission::Effect::Deny => Effect::Deny,
                                opencode_schema::permission::Effect::Ask => Effect::Ask,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for resource in &input.resources {
            match evaluate(&input.action, resource, &[&rules]).effect {
                Effect::Allow => {}
                Effect::Deny => return Err(PermissionError::Blocked { rules }),
                // Headless execution cannot silently grant an interactive permission.
                Effect::Ask => return Err(PermissionError::Declined),
            }
        }
        Ok(())
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
    use opencode_config::{Document, Entry};
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

    #[tokio::test]
    async fn actual_agent_rules_allow_read_deny_secret_and_decline_ask() {
        let root = tempfile::tempdir().unwrap();
        let config = CurrentConfig::from_entries(vec![Entry::Document(Document {
            path: None,
            info: json!({"permissions":[{"action":"read","resource":"secret.txt","effect":"deny"}]}),
        })]);
        let authorizer = ConfiguredAuthorization {
            agents: super::super::agents::build(&config, root.path(), root.path(), root.path()),
        };
        authorizer
            .assert(AssertInput::new(
                "session",
                "read",
                vec!["public.txt".into()],
            ))
            .await
            .unwrap();
        assert!(matches!(
            authorizer
                .assert(AssertInput::new(
                    "session",
                    "read",
                    vec!["secret.txt".into()]
                ))
                .await,
            Err(PermissionError::Blocked { .. })
        ));
        assert!(matches!(
            authorizer
                .assert(AssertInput::new("session", "read", vec![".env".into()]))
                .await,
            Err(PermissionError::Declined)
        ));
    }
}
