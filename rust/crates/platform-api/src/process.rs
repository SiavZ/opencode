//! Purpose: process execution contract: bounded capture, timeout/cancel with process-group and
//! descendant termination, streaming lines, joined teardown.
//! Scope: traits and values. Native implementation lives in platform-native.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::stream::BoxStream
//! |-- str::trim
//! |-- tokio::sync::mpsc::Sender
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/lib.rs
//! |-- platform-native/src/process/mod.rs
//! |-- platform-native/src/process/stream.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/format/mod.rs
//! |-- platform-native/src/vcs/run.rs
//! |-- platform-native/tests/process.rs
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/shell.rs
//! |-- tools/src/search/ripgrep.rs
//! |-- tools/src/shell/core.rs
//! `-- tools/src/shell/legacy.rs
//! Invariants: `run` returns only after the child and every descendant in its group are reaped;
//! truncation flags are set when more bytes arrived than the bound (baseline process.ts:121-139).
//! Contract tests: platform-native/tests/process.rs

use crate::error::PlatformError;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Variables added on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// Variables removed from the inherited environment.
    pub env_remove: Vec<String>,
    /// Start without inheriting the parent environment.
    pub env_clear: bool,
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// `describeCommand` from the baseline: `program arg1 arg2`.
    pub fn describe(&self) -> String {
        if self.args.is_empty() {
            return self.program.clone();
        }
        format!("{} {}", self.program, self.args.join(" "))
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Interleave stdout and stderr into `output`.
    pub combine_output: bool,
    pub max_output_bytes: Option<usize>,
    pub max_error_bytes: Option<usize>,
    pub timeout: Option<Duration>,
    pub cancel: Option<CancellationToken>,
    pub stdin: Option<Vec<u8>>,
    /// Grace period between SIGTERM and SIGKILL of the process group.
    pub kill_grace: Option<Duration>,
    /// Live copy of output chunks in arrival order: stdout and stderr interleaved when
    /// `combine_output` is set, otherwise stdout only. The sender applies backpressure: the reader
    /// waits for capacity. Capture bounds still apply to the returned buffers; the sink always
    /// sees the full stream. Closed receivers are ignored.
    pub output_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Start the child in its own process group (unix) so termination reaches descendants.
    /// Defaults to true in native adapters; `Some(false)` keeps the parent's group.
    pub detached: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunResult {
    pub command: String,
    pub exit_code: i32,
    /// Present when `combine_output` was requested.
    pub output: Option<Vec<u8>>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub output_truncated: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Signal number that terminated the process, unix only.
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    /// `AppProcessError`: message format `Command failed (exit N): cmd: detail`.
    #[error("{}", format_failure(.command, .exit_code, .stderr, .detail))]
    Failed {
        command: String,
        exit_code: Option<i32>,
        stderr: Option<String>,
        detail: Option<String>,
    },
    #[error("Timed out: {command}")]
    TimedOut { command: String },
    #[error("Aborted: {command}")]
    Cancelled { command: String },
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

fn format_failure(
    command: &str,
    exit_code: &Option<i32>,
    stderr: &Option<String>,
    detail: &Option<String>,
) -> String {
    let status = exit_code
        .map(|code| format!(" (exit {code})"))
        .unwrap_or_default();
    let detail = stderr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(detail.as_deref())
        .map(|d| format!(": {d}"))
        .unwrap_or_default();
    format!("Command failed{status}: {command}{detail}")
}

impl RunResult {
    /// `requireSuccess`.
    pub fn require_success(self) -> Result<Self, ProcessError> {
        self.require_exit_in(&[0])
    }

    /// `requireExitIn`.
    pub fn require_exit_in(self, codes: &[i32]) -> Result<Self, ProcessError> {
        if codes.contains(&self.exit_code) {
            return Ok(self);
        }
        Err(ProcessError::Failed {
            command: self.command,
            exit_code: Some(self.exit_code),
            stderr: Some(String::from_utf8_lossy(&self.stderr).into_owned()),
            detail: None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub include_stderr: bool,
    pub ok_exit_codes: Vec<i32>,
    pub max_error_bytes: Option<usize>,
    pub cancel: Option<CancellationToken>,
}

/// Process runner. Implementations spawn each command in its own process group and terminate
/// the whole group (and on Windows the job/tree) on timeout, cancellation or drop.
#[async_trait]
pub trait ProcessRunner: Send + Sync + std::fmt::Debug {
    async fn run(&self, command: &Command, options: RunOptions) -> Result<RunResult, ProcessError>;
    /// Non-empty output lines. Fails at the end when the exit code is not in `ok_exit_codes`
    /// (when non-empty).
    fn run_stream(
        &self,
        command: Command,
        options: StreamOptions,
    ) -> BoxStream<'static, Result<String, ProcessError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_matches_baseline() {
        let error = ProcessError::Failed {
            command: "git status".into(),
            exit_code: Some(128),
            stderr: Some(" fatal: no repo \n".into()),
            detail: None,
        };
        assert_eq!(
            error.to_string(),
            "Command failed (exit 128): git status: fatal: no repo"
        );
        assert_eq!(Command::new("ls").args(["-la", "x"]).describe(), "ls -la x");
    }
}
