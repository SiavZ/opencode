//! Purpose: an owned child server process: isolated process group, line-forwarded output with a
//! bounded recent-output buffer, readiness/health polling raced against exit/timeout/cancel, and a
//! joined graceful-stop-then-kill of the whole process tree.
//! Scope: generic over the launched program (local opencode sidecar, WSL `wsl.exe` sidecar, tests).
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/health.rs
//! |-- desktop-host/src/platform/process_tree.rs
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! |-- tokio::{process, sync::{oneshot, watch}, task::JoinHandle}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections/sidecar.rs
//! |-- desktop-host/src/platform/wsl/sidecar.rs
//! `-- desktop-host/tests/connections_live.rs
//! Invariants: one monitor task owns the `Child`; `stop` is idempotent, signals the tree, waits up to
//! the grace period, kills, and joins the monitor (which joins the output readers) before returning.
//! Contract tests: desktop-host/tests/connections_live.rs

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ConnectionError;
use super::health::{BasicAuth, check_health};
use crate::platform::process_tree::{self, ExitInfo};
use crate::platform::proxy::EnvMap;

/// Lines of recent output kept for startup failure messages (`wsl/sidecar.ts:55`).
pub const RECENT_OUTPUT_LINES: usize = 12;
/// Health poll interval (`server.ts:153`, `wsl/startup.ts:21`).
pub const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: OutputStream,
    pub text: String,
}

pub type LineSink = Arc<dyn Fn(OutputLine) + Send + Sync>;

/// How to launch a child server. `env` replaces the inherited environment entirely.
#[derive(Clone)]
pub struct ChildSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: EnvMap,
    pub cwd: Option<PathBuf>,
    /// Script written to stdin then closed (WSL `bash -se`); `None` gives a null stdin.
    pub stdin: Option<String>,
    pub url: String,
    pub auth: Option<BasicAuth>,
    pub stop_grace: Duration,
    pub on_line: Option<LineSink>,
}

impl std::fmt::Debug for ChildSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// Why a child failed to become healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthFailure {
    Exited { exit: ExitInfo, recent: Vec<String> },
    Timeout,
    Cancelled,
}

pub struct ChildServer {
    pid: Option<u32>,
    url: String,
    auth: Option<BasicAuth>,
    program: String,
    stop_tx: parking_lot::Mutex<Option<oneshot::Sender<Duration>>>,
    stop_grace: Duration,
    exit_rx: watch::Receiver<Option<ExitInfo>>,
    monitor: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    recent: Arc<parking_lot::Mutex<VecDeque<String>>>,
}

impl std::fmt::Debug for ChildServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildServer")
            .field("pid", &self.pid)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl ChildServer {
    /// Spawn the child in its own process group and start the owned monitor/reader tasks.
    pub async fn spawn(spec: ChildSpec) -> Result<Self, ConnectionError> {
        let program = spec.program.to_string_lossy().to_string();
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(&spec.env)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        process_tree::isolate(&mut command);
        let mut child = command.spawn().map_err(|error| ConnectionError::Spawn {
            program: program.clone(),
            message: error.to_string(),
        })?;
        if let (Some(script), Some(mut stdin)) = (&spec.stdin, child.stdin.take()) {
            // Failure to write means the child already exited; the monitor reports that exit.
            let _ = stdin.write_all(script.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let recent = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
        let readers: Vec<JoinHandle<()>> = [
            child
                .stdout
                .take()
                .map(|out| forward(out, OutputStream::Stdout, &spec.on_line, &recent)),
            child
                .stderr
                .take()
                .map(|err| forward(err, OutputStream::Stderr, &spec.on_line, &recent)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let pid = child.id();
        let (stop_tx, stop_rx) = oneshot::channel::<Duration>();
        let (exit_tx, exit_rx) = watch::channel(None);
        let monitor = tokio::spawn(async move {
            let exit = tokio::select! {
                status = child.wait() => {
                    // Leader exited on its own; sweep any orphaned descendants in its group.
                    if let Some(pid) = pid {
                        process_tree::signal_tree(pid, process_tree::TreeSignal::Kill).await;
                    }
                    status.ok().map(ExitInfo::from_status)
                }
                grace = stop_rx => match grace {
                    Ok(grace) => process_tree::terminate_tree(&mut child, grace).await,
                    // Handle dropped without stop: kill and reap so nothing outlives the owner.
                    Err(_) => process_tree::kill_tree(&mut child).await,
                },
            };
            for reader in readers {
                let abort = reader.abort_handle();
                if tokio::time::timeout(Duration::from_secs(2), reader)
                    .await
                    .is_err()
                {
                    abort.abort();
                }
            }
            let _ = exit_tx.send(Some(exit.unwrap_or(ExitInfo {
                code: None,
                signal: None,
            })));
        });
        Ok(Self {
            pid,
            url: spec.url,
            auth: spec.auth,
            program,
            stop_tx: parking_lot::Mutex::new(Some(stop_tx)),
            stop_grace: spec.stop_grace,
            exit_rx,
            monitor: tokio::sync::Mutex::new(Some(monitor)),
            recent,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn recent_output(&self) -> Vec<String> {
        self.recent.lock().iter().cloned().collect()
    }

    /// Exit info if the child has exited and been reaped.
    pub fn exit_info(&self) -> Option<ExitInfo> {
        self.exit_rx.borrow().clone()
    }

    /// Resolves when the child has exited and its tree has been swept.
    pub async fn exited(&self) -> ExitInfo {
        let mut rx = self.exit_rx.clone();
        loop {
            if let Some(exit) = rx.borrow_and_update().clone() {
                return exit;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone().unwrap_or(ExitInfo {
                    code: None,
                    signal: None,
                });
            }
        }
    }

    /// Poll health every 100ms until healthy, racing exit, `timeout` and `cancel`
    /// (`server.ts:142-162`, `wsl/sidecar.ts:63-87`).
    pub async fn wait_healthy(
        &self,
        client: &reqwest::Client,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<(), HealthFailure> {
        let poll = async {
            loop {
                tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
                if check_health(client, &self.url, self.auth.as_ref()).await {
                    return;
                }
            }
        };
        tokio::select! {
            () = poll => Ok(()),
            exit = self.exited() => Err(HealthFailure::Exited { exit, recent: self.recent_output() }),
            () = tokio::time::sleep(timeout) => Err(HealthFailure::Timeout),
            () = cancel.cancelled() => Err(HealthFailure::Cancelled),
        }
    }

    /// Graceful stop then joined kill of the process tree. Idempotent; later callers wait for the
    /// same join. Returns the exit info of the leader.
    pub async fn stop(&self) -> Option<ExitInfo> {
        if let Some(tx) = self.stop_tx.lock().take() {
            let _ = tx.send(self.stop_grace);
        }
        let mut monitor = self.monitor.lock().await;
        if let Some(handle) = monitor.take() {
            let _ = handle.await;
        }
        self.exit_info()
    }

    /// Immediate kill (no grace), joined.
    pub async fn kill(&self) -> Option<ExitInfo> {
        if let Some(tx) = self.stop_tx.lock().take() {
            let _ = tx.send(Duration::ZERO);
        }
        let mut monitor = self.monitor.lock().await;
        if let Some(handle) = monitor.take() {
            let _ = handle.await;
        }
        self.exit_info()
    }
}

fn forward<R: AsyncRead + Unpin + Send + 'static>(
    reader: R,
    stream: OutputStream,
    sink: &Option<LineSink>,
    recent: &Arc<parking_lot::Mutex<VecDeque<String>>>,
) -> JoinHandle<()> {
    let sink = sink.clone();
    let recent = recent.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let text = line.trim_end_matches('\r').to_string();
            if text.trim().is_empty() {
                continue;
            }
            {
                let mut recent = recent.lock();
                recent.push_back(format!("[{}] {}", stream.as_str(), text));
                while recent.len() > RECENT_OUTPUT_LINES {
                    recent.pop_front();
                }
            }
            if let Some(sink) = &sink {
                sink(OutputLine { stream, text });
            }
        }
    })
}
