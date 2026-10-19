//! Purpose: legacy `bash` shell tool (packages/opencode/src/tool/shell.ts): tree-sitter permission
//! scan with external_directory + bash approvals, streamed progress metadata, rolling in-memory
//! window, retained full output on overflow, tail truncation and timeout/abort shell metadata.
//! Scope: tool contract over `ProcessRunner`, `OutputRetention` and a shell-env port.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/path.rs
//! |-- platform-api/src/process.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/search/input.rs
//! |-- tools/src/shell/mod.rs
//! |-- tools/src/shell/output.rs
//! |-- tools/src/shell/prompt.rs
//! |-- tools/src/shell/scan.rs
//! `-- tools/src/shell/select.rs
//! External:
//! |-- async_trait::async_trait
//! |-- path::resolve
//! |-- schemas::legacy
//! |-- select::acceptable
//! |-- select::name
//! |-- select::ps
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::spawn
//! `-- tokio::sync::mpsc::channel
//! Used by:
//! rust/crates/
//! `-- tools/src/shell/mod.rs
//! Invariants: nothing spawns before both approvals; timeout fires at `timeout + 100` ms; abort and
//! timeout kill the process group (3 s grace) and keep collected output; the in-memory window is
//! bounded to 2 x maxBytes while the retained file receives every byte once overflowed.
//! Contract tests: tools/tests/shell_legacy.rs

use super::output::{OutputRetention, Utf8Decoder, append, preview, tail};
use super::prompt::{Limits, render};
use super::scan::{Instance, collect, globs};
use super::{schemas, select};
use crate::files::common::legacy_output;
use crate::search::input::{legacy_invalid, object};
use async_trait::async_trait;
use opencode_platform_api::path;
use opencode_platform_api::process::{Command, ProcessError, ProcessRunner, RunOptions};
use opencode_tool_api::{Tool, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// `plugin.trigger("shell.env", ...)` port: extra variables layered on the inherited environment.
#[async_trait]
pub trait ShellEnv: Send + Sync + std::fmt::Debug {
    async fn env(&self, cwd: &Path, session_id: &str, call_id: &str) -> BTreeMap<String, String>;
}

#[derive(Debug, Default)]
pub struct NoShellEnv;

#[async_trait]
impl ShellEnv for NoShellEnv {
    async fn env(&self, _: &Path, _: &str, _: &str) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
}

#[derive(Debug, Clone)]
pub struct LegacyShellConfig {
    /// Config `shell`; `None` falls back to `$SHELL` via `Shell.acceptable`.
    pub shell: Option<String>,
    /// `RuntimeFlags.bashDefaultTimeoutMs ?? 120000`.
    pub default_timeout_ms: u64,
    /// `Global.Path.tmp` for the description.
    pub tmp: String,
    /// `os.homedir()` for `~` expansion in the scan.
    pub home: PathBuf,
    pub directory: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LegacyShellTool {
    runner: Arc<dyn ProcessRunner>,
    retention: Arc<dyn OutputRetention>,
    env: Arc<dyn ShellEnv>,
    config: LegacyShellConfig,
    shell: String,
    description: String,
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn failed(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(error.to_string())
}

impl LegacyShellTool {
    pub fn new(
        runner: Arc<dyn ProcessRunner>,
        retention: Arc<dyn OutputRetention>,
        env: Arc<dyn ShellEnv>,
        config: LegacyShellConfig,
    ) -> Self {
        let env_shell = std::env::var("SHELL").ok();
        let shell = select::acceptable(config.shell.as_deref(), env_shell.as_deref());
        let (max_lines, max_bytes) = retention.limits();
        let description = render(
            &select::name(&shell),
            platform(),
            &config.tmp,
            Limits {
                max_lines,
                max_bytes,
            },
            config.default_timeout_ms,
        );
        Self {
            runner,
            retention,
            env,
            config,
            shell,
            description,
        }
    }

    pub fn shell(&self) -> &str {
        &self.shell
    }

    async fn ask(
        &self,
        command: &str,
        cwd: &Path,
        context: &ToolContext,
    ) -> Result<(), ToolFailure> {
        let instance = Instance {
            directory: self.config.directory.clone(),
            worktree: self.config.worktree.clone(),
        };
        let mut scan = collect(
            command,
            cwd,
            &select::name(&self.shell),
            &instance,
            &self.config.home,
        )
        .map_err(failed)?;
        if !instance.contains(cwd) {
            scan.dirs.add(cwd.to_string_lossy().into_owned());
        }
        if !scan.dirs.is_empty() {
            let patterns = globs(&scan);
            let metadata =
                json!({"command": command, "directories": scan.dirs.items(), "patterns": patterns});
            context
                .authorize(
                    "external_directory",
                    patterns.clone(),
                    Some(patterns),
                    metadata.as_object().cloned(),
                )
                .await?;
        }
        if scan.patterns.is_empty() {
            return Ok(());
        }
        context
            .authorize(
                "bash",
                scan.patterns.items().to_vec(),
                Some(scan.always.items().to_vec()),
                json!({"command": command}).as_object().cloned(),
            )
            .await?;
        Ok(())
    }

    fn command(&self, command: &str, cwd: &Path, env: BTreeMap<String, String>) -> Command {
        let base = if cfg!(windows) && select::ps(&self.shell) {
            Command::new(&self.shell).args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ])
        } else if cfg!(windows) && select::name(&self.shell) == "cmd" {
            Command::new(&self.shell).args(["/d", "/s", "/c", command])
        } else {
            Command::new(&self.shell).args(["-c", command])
        };
        let mut base = base.cwd(cwd);
        base.env = env;
        base
    }

    async fn run(
        &self,
        command: &str,
        cwd: &Path,
        timeout: u64,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let (max_lines, max_bytes) = self.retention.limits();
        let keep = max_bytes * 2;
        let progress = |output: &str| {
            let mut metadata = Map::new();
            metadata.insert("output".into(), json!(output));
            context.progress.update(None, &metadata);
        };
        progress("");
        let env = self
            .env
            .env(cwd, &context.session_id, &context.tool_call_id)
            .await;
        let process = self.command(command, cwd, env);
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let options = RunOptions {
            combine_output: true,
            max_output_bytes: Some(0),
            timeout: Some(Duration::from_millis(timeout + 100)),
            cancel: Some(context.cancel.clone()),
            kill_grace: Some(Duration::from_secs(3)),
            output_sink: Some(sender),
            ..RunOptions::default()
        };
        let runner = self.runner.clone();
        let task = tokio::spawn(async move { runner.run(&process, options).await });

        let mut decoder = Utf8Decoder::default();
        let mut window: VecDeque<String> = VecDeque::new();
        let (mut used, mut cut) = (0usize, false);
        let (mut full, mut last) = (String::new(), String::new());
        let mut file: Option<PathBuf> = None;
        let mut consume = async |chunk: String| -> Result<(), ToolFailure> {
            if chunk.is_empty() {
                return Ok(());
            }
            used += chunk.len();
            window.push_back(chunk.clone());
            while used > keep && window.len() > 1 {
                if let Some(item) = window.pop_front() {
                    used -= item.len();
                    cut = true;
                }
            }
            last = preview(&(last.clone() + &chunk));
            match &file {
                Some(target) => append(target, &chunk).await.map_err(failed)?,
                None => {
                    full.push_str(&chunk);
                    if full.len() > max_bytes {
                        file = Some(self.retention.write(&full).await.map_err(failed)?);
                        cut = true;
                        full.clear();
                    }
                }
            }
            progress(&last);
            Ok(())
        };
        while let Some(chunk) = receiver.recv().await {
            consume(decoder.push(&chunk)).await?;
        }
        consume(decoder.finish()).await?;
        let result = task.await.map_err(failed)?;
        let (code, expired, aborted) = match result {
            Ok(result) if result.signal.is_some() => {
                return Err(failed(format!(
                    "Process interrupted due to receipt of signal: {:?}",
                    result.signal
                )));
            }
            Ok(result) => (Some(result.exit_code), false, false),
            Err(ProcessError::TimedOut { .. }) => (None, true, false),
            Err(ProcessError::Cancelled { .. }) => (None, false, true),
            Err(error) => return Err(failed(error)),
        };

        let mut meta = Vec::new();
        if expired {
            meta.push(format!("shell tool terminated command after exceeding timeout {timeout} ms. If this command is expected to take longer and is not waiting for interactive input, retry with a larger timeout value in milliseconds."));
        }
        if aborted {
            meta.push("User aborted the command".to_string());
        }
        let raw: String = window.iter().map(String::as_str).collect();
        let (end, end_cut) = tail(&raw, max_lines, max_bytes);
        cut = cut || end_cut;
        if file.is_none() && end_cut {
            file = Some(self.retention.write(&raw).await.map_err(failed)?);
        }
        let mut output = if end.is_empty() {
            "(no output)".to_string()
        } else {
            end
        };
        if let (true, Some(target)) = (cut, &file) {
            output = format!(
                "...output truncated...\n\nFull output saved to: {}\n\n{output}",
                target.display()
            );
        }
        if !meta.is_empty() {
            output += &format!(
                "\n\n<shell_metadata>\n{}\n</shell_metadata>",
                meta.join("\n")
            );
        }
        let mut metadata = Map::new();
        metadata.insert(
            "output".into(),
            json!(if last.is_empty() {
                preview(&output)
            } else {
                last
            }),
        );
        metadata.insert("exit".into(), code.map_or(Value::Null, Value::from));
        metadata.insert("truncated".into(), json!(cut));
        if let (true, Some(target)) = (cut, &file) {
            metadata.insert("outputPath".into(), json!(target.to_string_lossy()));
        }
        Ok(legacy_output(
            command.to_string(),
            Value::Object(metadata),
            output,
        ))
    }
}

#[async_trait]
impl Tool for LegacyShellTool {
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        schemas::legacy()
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let invalid = |detail| legacy_invalid("bash", detail);
        let object = object(&input).map_err(invalid)?;
        let command = object.string("command").map_err(invalid)?;
        let timeout = object.optional_positive("timeout", None).map_err(invalid)?;
        let workdir = object.optional_string("workdir").map_err(invalid)?;
        let cwd = match workdir {
            Some(workdir) => path::resolve(&self.config.directory, Path::new(&workdir)),
            None => self.config.directory.clone(),
        };
        let timeout = timeout.unwrap_or(self.config.default_timeout_ms);
        self.ask(&command, &cwd, context).await?;
        self.run(&command, &cwd, timeout, context).await
    }
}
