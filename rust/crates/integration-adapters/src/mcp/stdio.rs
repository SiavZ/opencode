//! Purpose: stdio MCP transport: spawn the configured command with merged env and cwd, NDJSON
//! framing, graceful close (stdin EOF, SIGTERM, SIGKILL) and descendant process discovery.
//! Scope: one child process. Reader and stderr drain tasks are owned and joined on close.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! |-- nix::sys::signal::{Signal, kill}
//! |-- serde_json::Value
//! `-- tokio::process::{Child, ChildStdin, Command}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/connect.rs
//! `-- integration-adapters/src/mcp/mod.rs
//! Invariants: matches SDK StdioClientTransport: env = sudo-style defaults + caller env, stderr
//! piped (drained to tracing), lines split on `\n` with trailing `\r` stripped, unparsable lines
//! reported and skipped, close waits 2s per escalation step.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};

use super::transport::{INBOUND_CAPACITY, Inbound, Transport, TransportError};

/// Baseline SDK `DEFAULT_INHERITED_ENV_VARS` (non-Windows).
const DEFAULT_INHERITED_ENV_VARS: [&str; 6] = ["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];
const CLOSE_STEP: Duration = Duration::from_secs(2);

pub struct StdioParams {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    /// Full child environment overlay (the service passes process env + BUN_BE_BUN + config).
    pub env: BTreeMap<String, String>,
}

pub struct StdioTransport {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pid: Option<u32>,
    tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl StdioTransport {
    pub fn spawn(params: StdioParams) -> Result<(Self, mpsc::Receiver<Inbound>), TransportError> {
        let mut cmd = Command::new(&params.command);
        cmd.args(&params.args)
            .current_dir(&params.cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in DEFAULT_INHERITED_ENV_VARS {
            if let Ok(v) = std::env::var(key)
                && !v.starts_with("()")
            {
                cmd.env(key, v);
            }
        }
        cmd.envs(&params.env);
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Failed(spawn_error(&params.command, &e)))?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel(INBOUND_CAPACITY);
        let mut tasks = Vec::new();
        if let Some(stdout) = stdout {
            tasks.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).split(b'\n');
                while let Ok(Some(line)) = lines.next_segment().await {
                    let text = String::from_utf8_lossy(&line);
                    let text = text.strip_suffix('\r').unwrap_or(&text);
                    match serde_json::from_str::<Value>(text) {
                        Ok(v) if v.is_object() || v.is_array() => {
                            if tx.send(Inbound::Message(v)).await.is_err() {
                                return;
                            }
                        }
                        _ => tracing::warn!(line = %text, "MCP stdio: invalid JSON-RPC line"),
                    }
                }
                let _ = tx.send(Inbound::Closed("Connection closed".into())).await;
            }));
        }
        if let Some(stderr) = stderr {
            let command = params.command.clone();
            tasks.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(%command, %line, "MCP server stderr");
                }
            }));
        }
        Ok((
            Self {
                stdin: Mutex::new(stdin),
                child: Mutex::new(Some(child)),
                pid,
                tasks: StdMutex::new(tasks),
            },
            rx,
        ))
    }
}

fn spawn_error(command: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("spawn {command} ENOENT"),
        std::io::ErrorKind::PermissionDenied => format!("spawn {command} EACCES"),
        _ => format!("spawn {command}: {e}"),
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send(&self, message: Value) -> Result<(), TransportError> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or(TransportError::Closed)?;
        let mut line =
            serde_json::to_vec(&message).map_err(|e| TransportError::Failed(e.to_string()))?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|e| TransportError::Failed(e.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|e| TransportError::Failed(e.to_string()))
    }

    async fn close(&self) {
        drop(self.stdin.lock().await.take());
        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            let exited = tokio::time::timeout(CLOSE_STEP, child.wait()).await.is_ok();
            if !exited {
                if let Some(pid) = child.id() {
                    signal(pid, nix::sys::signal::Signal::SIGTERM);
                }
                if tokio::time::timeout(CLOSE_STEP, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            }
        }
        let tasks: Vec<_> = self
            .tasks
            .lock()
            .map(|mut t| t.drain(..).collect())
            .unwrap_or_default();
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }
}

pub fn signal(pid: u32, sig: nix::sys::signal::Signal) {
    if let Ok(pid) = i32::try_from(pid) {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), sig);
    }
}

/// Breadth-first descendant pids via `pgrep -P` (baseline index.ts:418-440). Errors yield [].
pub async fn descendants(pid: u32) -> Vec<u32> {
    if cfg!(windows) {
        return Vec::new();
    }
    let mut pids: Vec<u32> = Vec::new();
    let mut queue = vec![pid];
    let mut index = 0;
    while index < queue.len() {
        let current = queue[index];
        index += 1;
        let Ok(out) = Command::new("pgrep")
            .args(["-P", &current.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
        else {
            return Vec::new();
        };
        for tok in String::from_utf8_lossy(&out.stdout).split('\n') {
            if let Ok(cpid) = tok.trim().parse::<u32>()
                && !pids.contains(&cpid)
            {
                pids.push(cpid);
                queue.push(cpid);
            }
        }
    }
    pids
}
