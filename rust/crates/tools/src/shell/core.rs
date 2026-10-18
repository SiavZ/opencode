//! Purpose: core V2 `bash` tool (packages/core/src/tool/bash.ts): Location-resolved workdir with
//! external_directory approval, advisory external-argument warnings, bash approval on the exact
//! command, directory check after approval, one shell process with timeout, cancellation and a
//! 1 MiB combined capture bound.
//! Scope: tool contract over `ProcessRunner` and `LocationMutation`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/path.rs
//! |-- platform-api/src/process.rs
//! |-- tool-api/src/context.rs
//! |-- tools/src/search/input.rs
//! `-- tools/src/shell/mod.rs
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! `-- tools/src/shell/mod.rs
//! Invariants: approvals run in order external_directory, bash; nothing spawns after a denial or
//! when the workdir is not a directory at spawn time. Every failure maps to
//! `Unable to execute command: <command>` (baseline Effect.mapError over the whole body).
//! Contract tests: tools/tests/shell_core.rs

use super::schemas;
use crate::search::input::object;
use async_trait::async_trait;
use opencode_platform_api::fs::{LocationMutation, ResolveInput, ResolveKind};
use opencode_platform_api::path;
use opencode_platform_api::process::{Command, ProcessError, ProcessRunner, RunOptions};
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const DEFAULT_TIMEOUT_MS: u64 = 2 * 60 * 1_000;
pub const MAX_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

pub fn description() -> String {
    format!(
        "Execute one shell command string with the host user's filesystem, process, and network authority. The active Location is the default working directory. Relative workdir values resolve from that Location. External workdir values require external_directory approval; best-effort command-argument path warnings are advisory only. Timeout values are milliseconds (default: {DEFAULT_TIMEOUT_MS}; maximum: {MAX_TIMEOUT_MS}). Uses the configured shell when set; otherwise uses /bin/sh on POSIX and COMSPEC or cmd.exe on Windows."
    )
}

#[derive(Debug, Clone)]
pub struct CoreBashTool {
    runner: Arc<dyn ProcessRunner>,
    location: Arc<dyn LocationMutation>,
    /// Merged config `shell`, when set.
    shell: Option<String>,
    description: String,
}

/// Process result after the timeout mapping.
struct Ran {
    output: String,
    exit: Option<i32>,
    truncated: bool,
    timeout: bool,
}

fn default_shell() -> String {
    if cfg!(windows) {
        return std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    }
    "/bin/sh".into()
}

fn shell_tokens(command: &str) -> Vec<&str> {
    static TOKENS: OnceLock<regex::Regex> = OnceLock::new();
    TOKENS
        .get_or_init(|| regex::Regex::new(r#"(?:[^\s"']+|"[^"]*"|'[^']*')+"#).expect("valid"))
        .find_iter(command)
        .map(|found| found.as_str())
        .collect()
}

fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        return &value[1..value.len() - 1];
    }
    value
}

/// `FSUtil.resolve`: realpath when it exists, otherwise the normalized absolute path.
fn real(value: &Path) -> PathBuf {
    std::fs::canonicalize(value).unwrap_or_else(|_| path::normalize(value))
}

/// Token-based advisory scan (`externalCommandDirectories`).
pub fn external_command_directories(command: &str, cwd: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in shell_tokens(command) {
        let value = unquote(token).trim_end_matches([';', ',', '|', '&']);
        if !Path::new(value).is_absolute() {
            continue;
        }
        let resolved = real(Path::new(value));
        if path::contains(cwd, &resolved) {
            continue;
        }
        let parent = resolved.parent().map(Path::to_path_buf).unwrap_or(resolved);
        let directory = real(&parent).to_string_lossy().into_owned();
        if !out.contains(&directory) {
            out.push(directory);
        }
    }
    out
}

/// `modelOutput`.
pub fn model_text(warnings: &[String], exit: Option<i32>, timeout: bool) -> String {
    let block = if warnings.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = warnings.iter().map(|w| format!("- {w}")).collect();
        format!("Warnings:\n{}\n\n", lines.join("\n"))
    };
    if timeout {
        return format!("{block}Command timed out before completion.");
    }
    let exit = exit.map_or("undefined".to_string(), |code| code.to_string());
    format!("{block}Command exited with code {exit}.")
}

impl CoreBashTool {
    pub fn new(
        runner: Arc<dyn ProcessRunner>,
        location: Arc<dyn LocationMutation>,
        shell: Option<String>,
    ) -> Self {
        Self {
            runner,
            location,
            shell,
            description: description(),
        }
    }

    async fn run(
        &self,
        command: &str,
        workdir: Option<&str>,
        timeout: u64,
        context: &ToolContext,
    ) -> Result<(Ran, Vec<String>), String> {
        let target = self
            .location
            .resolve(ResolveInput {
                path: workdir.unwrap_or(".").into(),
                kind: Some(ResolveKind::Directory),
            })
            .await
            .map_err(|error| error.to_string())?;
        if let Some(external) = &target.external_directory {
            let (action, resources, save) = external.permission();
            context
                .authorize(action, resources, Some(save), None)
                .await
                .map_err(|error| error.to_string())?;
        }
        let warnings: Vec<String> = external_command_directories(command, &target.canonical)
            .into_iter()
            .map(|directory| {
                format!(
                    "Command argument references external directory {}. Bash runs with host-user filesystem, process, and network authority; this scan is advisory only.",
                    path::slash(&Path::new(&directory).join("*").to_string_lossy())
                )
            })
            .collect();
        context
            .authorize(
                "bash",
                vec![command.into()],
                Some(vec![command.into()]),
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        // The directory check runs after approval: an approval prompt can take arbitrarily long.
        if !tokio::fs::metadata(&target.canonical)
            .await
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err(format!(
                "Working directory is not a directory: {}",
                target.canonical.display()
            ));
        }
        let shell = self.shell.clone().unwrap_or_else(default_shell);
        let flag = if super::select::name(&shell) == "cmd" {
            "/c"
        } else {
            "-c"
        };
        let process = Command::new(shell)
            .args([flag, command])
            .cwd(&target.canonical);
        let result = self
            .runner
            .run(
                &process,
                RunOptions {
                    combine_output: true,
                    max_output_bytes: Some(MAX_CAPTURE_BYTES),
                    timeout: Some(Duration::from_millis(timeout)),
                    cancel: Some(context.cancel.clone()),
                    kill_grace: Some(Duration::from_secs(3)),
                    ..RunOptions::default()
                },
            )
            .await;
        let ran = match result {
            Err(ProcessError::TimedOut { .. }) => Ran {
                output: format!(
                    "Command exceeded timeout of {timeout} ms. Retry with a larger timeout if the command is expected to take longer."
                ),
                exit: None,
                truncated: false,
                timeout: true,
            },
            Err(error) => return Err(error.to_string()),
            // A signal-terminated shell has no exit code: the baseline exitCode effect fails.
            Ok(result) if result.signal.is_some() => {
                return Err(format!(
                    "Process interrupted due to receipt of signal: {:?}",
                    result.signal
                ));
            }
            Ok(result) => {
                let text = String::from_utf8_lossy(result.output.as_deref().unwrap_or_default())
                    .into_owned();
                let text = if text.is_empty() {
                    "(no output)".to_string()
                } else {
                    text
                };
                Ran {
                    output: if result.output_truncated {
                        format!(
                            "{text}\n\n[output capture truncated at the in-memory safety limit]"
                        )
                    } else {
                        text
                    },
                    exit: Some(result.exit_code),
                    truncated: result.output_truncated,
                    timeout: false,
                }
            }
        };
        Ok((ran, warnings))
    }
}

#[async_trait]
impl Tool for CoreBashTool {
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        schemas::core("input")
    }
    fn output_schema(&self) -> Option<Value> {
        Some(schemas::core("output"))
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let object = object(&input).map_err(ToolFailure::invalid_input)?;
        let command = object
            .string("command")
            .map_err(ToolFailure::invalid_input)?;
        let workdir = object
            .optional_string("workdir")
            .map_err(ToolFailure::invalid_input)?;
        let timeout = object
            .optional_positive("timeout", Some(MAX_TIMEOUT_MS))
            .map_err(ToolFailure::invalid_input)?
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let (ran, warnings) = self
            .run(&command, workdir.as_deref(), timeout, context)
            .await
            .map_err(|_| ToolFailure::new(format!("Unable to execute command: {command}")))?;
        let mut structured = Map::new();
        if let Some(exit) = ran.exit {
            structured.insert("exit".into(), json!(exit));
        }
        structured.insert("truncated".into(), json!(ran.truncated));
        if ran.timeout {
            structured.insert("timeout".into(), json!(true));
        }
        Ok(ToolOutput::new(
            Value::Object(structured),
            vec![
                ToolContent::text(ran.output),
                ToolContent::text(model_text(&warnings, ran.exit, ran.timeout)),
            ],
        ))
    }
}
