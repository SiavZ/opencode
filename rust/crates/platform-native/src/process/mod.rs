//! Purpose: native `ProcessRunner` (packages/core/src/process.ts + cross-spawn-spawner.ts): spawn in
//! a dedicated process group, bounded capture, live output sink, timeout and cancellation with
//! group termination (SIGTERM, grace, SIGKILL), and joined teardown of reader tasks.
//! Scope: tokio child processes. Line streaming lives in `stream.rs`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/error.rs
//! |-- platform-api/src/process.rs
//! |-- platform-native/src/process/capture.rs
//! |-- platform-native/src/process/kill.rs
//! `-- platform-native/src/process/stream.rs
//! External:
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! |-- platform-native/src/lib.rs
//! `-- platform-native/tests/process.rs
//! Invariants: `run` returns only after the leader is reaped and reader tasks are joined. Timeout,
//! cancellation, drop and non-zero exit terminate the whole group, mirroring the baseline spawner
//! finalizer; a zero exit leaves detached background jobs that no longer hold the pipes alone.
//! Stdin delivery runs inside the same timeout/cancellation lifetime, with no detached writer task.
//! Contract tests: platform-native/tests/process.rs

pub mod capture;
pub mod kill;
pub mod stream;

use async_trait::async_trait;
use futures::stream::BoxStream;
use opencode_platform_api::error::{PlatformError, PlatformErrorKind};
use opencode_platform_api::process::{
    Command, ProcessError, ProcessRunner, RunOptions, RunResult, StreamOptions,
};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

/// Bound on how long teardown waits for pipe readers after the group was terminated. Readers can
/// outlive the group only when a descendant escaped it (setsid) while holding the pipe.
const DRAIN: Duration = Duration::from_millis(500);

#[derive(Debug, Default, Clone)]
pub struct NativeProcessRunner;

impl NativeProcessRunner {
    pub fn new() -> Self {
        Self
    }
}

pub(crate) fn spawn_error(command: &Command, error: &std::io::Error) -> ProcessError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => PlatformErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => PlatformErrorKind::PermissionDenied,
        _ => PlatformErrorKind::Io,
    };
    ProcessError::Platform(PlatformError::new(
        kind,
        "spawn",
        command.describe(),
        error.to_string(),
    ))
}

/// Build a tokio command with the baseline spawner defaults: own process group unless disabled.
pub(crate) fn prepare(
    command: &Command,
    detached: bool,
    stdin: bool,
) -> Result<tokio::process::Command, ProcessError> {
    if let Some(cwd) = &command.cwd
        && !cwd.exists()
    {
        // cross-spawn-spawner.ts `cwd`: fs.access(cwd) before spawning.
        return Err(ProcessError::Platform(PlatformError::new(
            PlatformErrorKind::NotFound,
            "access",
            cwd.to_string_lossy(),
            "no such file or directory",
        )));
    }
    let mut out = tokio::process::Command::new(&command.program);
    out.args(&command.args);
    if let Some(cwd) = &command.cwd {
        out.current_dir(cwd);
    }
    if command.env_clear {
        out.env_clear();
    }
    for key in &command.env_remove {
        out.env_remove(key);
    }
    out.envs(&command.env);
    out.stdin(if stdin { Stdio::piped() } else { Stdio::null() });
    out.stdout(Stdio::piped()).stderr(Stdio::piped());
    out.kill_on_drop(true);
    #[cfg(unix)]
    if detached {
        out.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = detached;
    Ok(out)
}

#[cfg(unix)]
pub(crate) fn exit_parts(status: std::process::ExitStatus) -> (i32, Option<i32>) {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => (code, None),
        (None, signal) => (-1, signal),
    }
}

#[cfg(not(unix))]
pub(crate) fn exit_parts(status: std::process::ExitStatus) -> (i32, Option<i32>) {
    (status.code().unwrap_or(-1), None)
}

async fn join_readers(readers: Vec<JoinHandle<std::io::Result<()>>>, bounded: bool) {
    for mut reader in readers {
        // Finished handles may already have been awaited; polling them again would panic.
        if reader.is_finished() {
            continue;
        }
        if !bounded {
            let _ = (&mut reader).await;
            continue;
        }
        if tokio::time::timeout(DRAIN, &mut reader).await.is_err() {
            reader.abort();
            let _ = reader.await;
        }
    }
}

enum Outcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

#[async_trait]
impl ProcessRunner for NativeProcessRunner {
    async fn run(
        &self,
        command: &Command,
        mut options: RunOptions,
    ) -> Result<RunResult, ProcessError> {
        let description = command.describe();
        let detached = options.detached.unwrap_or(true);
        let mut child = prepare(command, detached, options.stdin.is_some())?
            .spawn()
            .map_err(|error| spawn_error(command, &error))?;
        let target = kill::Target {
            pid: child.id().unwrap_or_default(),
            group: detached,
        };
        let mut guard = kill::GroupGuard::new(target);
        let grace = options.kill_grace.unwrap_or(kill::DEFAULT_GRACE);

        // Keep the writer in the run future instead of a spawned task. Dropping this
        // future closes stdin on every error, cancellation and caller-drop path.
        let stdin = child.stdin.take();
        let bytes = options.stdin.take();
        let writer = async move {
            if let (Some(bytes), Some(mut stdin)) = (bytes, stdin) {
                let _ = stdin.write_all(&bytes).await;
            }
        };
        let (out, err) = if options.combine_output {
            let combined = capture::shared(options.max_output_bytes);
            (combined.clone(), combined)
        } else {
            (
                capture::shared(options.max_output_bytes),
                capture::shared(options.max_error_bytes),
            )
        };
        let mut readers = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            readers.push(tokio::spawn(capture::pump(
                pipe,
                out.clone(),
                options.output_sink.clone(),
            )));
        }
        if let Some(pipe) = child.stderr.take() {
            // Without combined output the sink is a stdout stream; stderr stays bounded capture.
            let sink = options
                .output_sink
                .clone()
                .filter(|_| options.combine_output);
            readers.push(tokio::spawn(capture::pump(pipe, err.clone(), sink)));
        }
        drop(options.output_sink);

        let cancel = options.cancel.clone().unwrap_or_default();
        let timeout = options.timeout;
        let outcome = {
            let collect = async {
                let status = child.wait().await;
                // Baseline collects streams and exit code together: pipes must reach EOF.
                for reader in readers.iter_mut() {
                    if !reader.is_finished() {
                        let _ = reader.await;
                    }
                }
                status
            };
            let finished = async { tokio::join!(collect, writer).0 };
            tokio::select! {
                status = finished => match status {
                    Ok(status) => Outcome::Exited(status),
                    Err(error) => return Err(spawn_error(command, &error)),
                },
                _ = async {
                    match timeout {
                        Some(duration) => tokio::time::sleep(duration).await,
                        None => std::future::pending().await,
                    }
                } => Outcome::TimedOut,
                _ = cancel.cancelled() => Outcome::Cancelled,
            }
        };
        let status = match outcome {
            Outcome::Exited(status) => {
                let (code, signal) = exit_parts(status);
                if (code != 0 || signal.is_some()) && kill::alive(target) {
                    kill::terminate(&mut child, target, grace).await;
                }
                guard.disarm();
                join_readers(readers, false).await;
                status
            }
            other => {
                kill::terminate(&mut child, target, grace).await;
                guard.disarm();
                join_readers(readers, true).await;
                return Err(match other {
                    Outcome::TimedOut => ProcessError::TimedOut {
                        command: description,
                    },
                    _ => ProcessError::Cancelled {
                        command: description,
                    },
                });
            }
        };
        let (exit_code, signal) = exit_parts(status);
        let out = capture::take(&out);
        if options.combine_output {
            return Ok(RunResult {
                command: description,
                exit_code,
                output_truncated: out.truncated,
                output: Some(out.bytes),
                signal,
                ..RunResult::default()
            });
        }
        let err = capture::take(&err);
        Ok(RunResult {
            command: description,
            exit_code,
            output: None,
            stdout: out.bytes,
            stderr: err.bytes,
            output_truncated: false,
            stdout_truncated: out.truncated,
            stderr_truncated: err.truncated,
            signal,
        })
    }

    fn run_stream(
        &self,
        command: Command,
        options: StreamOptions,
    ) -> BoxStream<'static, Result<String, ProcessError>> {
        stream::lines(command, options)
    }
}
