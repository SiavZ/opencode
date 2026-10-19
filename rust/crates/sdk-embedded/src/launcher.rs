//! Purpose: launch an `opencode serve` subprocess and wait for stdout readiness, or launch the
//! TUI attached to the terminal. Port of packages/sdk/js/src/server.ts:22-134.
//! Scope: one child process per handle, owned and joined by the handle.
//! Uses:
//! rust/crates/
//! `-- sdk-embedded/src/process.rs
//! External:
//! |-- char::is_whitespace
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- serde_json::Value
//! |-- serde_json::Value::String
//! |-- serde_json::Value::to_string
//! |-- thiserror::Error
//! |-- tokio::io::AsyncBufReadExt
//! |-- tokio::io::AsyncRead
//! |-- tokio::io::BufReader
//! |-- tokio::pin
//! |-- tokio::process::Child
//! |-- tokio::process::Command
//! |-- tokio::select
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc
//! |-- tokio::task::JoinHandle
//! |-- tokio::time::sleep
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- sdk-embedded/src/lib.rs
//! `-- sdk-embedded/tests/launcher.rs
//! Invariants: readiness is the first complete stdout line starting with
//! `opencode server listening`, the URL is the first `on <http(s)://...>` token. Timeout, parse
//! failure, abort and early exit all stop the child before returning. Output pipes keep draining
//! after readiness so the server never blocks on a full pipe.
//! Contract tests: sdk-embedded/tests/launcher.rs

use std::ffi::OsString;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::process::{Tree, isolate, stop};

const READY_PREFIX: &str = "opencode server listening";

/// Options of `createOpencodeServer` (server.ts:5-11) plus the program to run.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Executable resolved through `PATH`, `opencode` by default.
    pub program: OsString,
    pub hostname: String,
    pub port: u16,
    pub timeout: Duration,
    /// Serialized into `OPENCODE_CONFIG_CONTENT`, `{}` when absent.
    pub config: Option<serde_json::Value>,
    /// Abort signal: cancelling before or after readiness stops the server.
    pub cancel: Option<CancellationToken>,
}

impl Default for ServerOptions {
    /// Defaults of server.ts:23-30.
    fn default() -> Self {
        Self {
            program: "opencode".into(),
            hostname: "127.0.0.1".into(),
            port: 4096,
            timeout: Duration::from_millis(5000),
            config: None,
            cancel: None,
        }
    }
}

/// Options of `createOpencodeTui` (server.ts:13-20) plus the program to run.
#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub program: OsString,
    pub project: Option<String>,
    pub model: Option<String>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub config: Option<serde_json::Value>,
    pub cancel: Option<CancellationToken>,
}

impl Default for TuiOptions {
    fn default() -> Self {
        Self {
            program: "opencode".into(),
            project: None,
            model: None,
            session: None,
            agent: None,
            config: None,
            cancel: None,
        }
    }
}

/// Launch failures, messages match the Error messages of server.ts.
#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("{0}")]
    Spawn(#[source] std::io::Error),
    #[error("Timeout waiting for server to start after {}ms", .0.as_millis())]
    Timeout(Duration),
    #[error("Failed to parse server url from output: {0}")]
    ParseUrl(String),
    /// `code` is `None` when the child was killed by a signal (`null` in Node).
    #[error("Server exited with code {}{}", code.map_or("null".to_string(), |c| c.to_string()), exit_output(.output))]
    Exited { code: Option<i32>, output: String },
    /// The abort token fired (server.ts:91-94 rejects with the signal reason).
    #[error("aborted")]
    Aborted,
}

fn exit_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }
    format!("\nServer output: {output}")
}

/// Running server child. Dropping it stops the child, `close` stops it and joins the supervisor.
#[derive(Debug)]
pub struct ServerHandle {
    pub url: String,
    close: CancellationToken,
    supervisor: Option<JoinHandle<Option<ExitStatus>>>,
}

impl ServerHandle {
    /// `close()` of server.ts:98-101, joined: returns once the child has exited.
    pub async fn close(mut self) -> Option<ExitStatus> {
        self.close.cancel();
        let supervisor = self.supervisor.take()?;
        supervisor.await.ok().flatten()
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.close.cancel();
    }
}

/// Port of `createOpencodeServer` (server.ts:22-103).
pub async fn create_opencode_server(options: ServerOptions) -> Result<ServerHandle, LaunchError> {
    let mut command = Command::new(&options.program);
    command
        .arg("serve")
        .arg(format!("--hostname={}", options.hostname))
        .arg(format!("--port={}", options.port));
    if let Some(level) = log_level(options.config.as_ref()) {
        command.arg(format!("--log-level={level}"));
    }
    command
        .env(
            "OPENCODE_CONFIG_CONTENT",
            config_content(options.config.as_ref()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    isolate(&mut command);
    let cancel = options.cancel.unwrap_or_default();
    if cancel.is_cancelled() {
        return Err(LaunchError::Aborted);
    }
    let mut child = command.spawn().map_err(LaunchError::Spawn)?;
    let (lines, mut rx) = mpsc::channel::<Line>(64);
    let pumps = [
        tokio::spawn(pump(child.stdout.take(), Stream::Stdout, lines.clone())),
        tokio::spawn(pump(child.stderr.take(), Stream::Stderr, lines)),
    ];
    let deadline = tokio::time::sleep(options.timeout);
    tokio::pin!(deadline);
    let mut output = String::new();
    let failure = loop {
        tokio::select! {
            line = rx.recv() => {
                let Some(line) = line else {
                    // Both pipes closed: the exit branch reports the status.
                    let status = child.wait().await.ok();
                    break LaunchError::Exited { code: status.and_then(|s| s.code()), output };
                };
                output.push_str(&line.text);
                if line.stream == Stream::Stderr || !line.text.starts_with(READY_PREFIX) {
                    continue;
                }
                let Some(url) = parse_url(&line.text) else {
                    break LaunchError::ParseUrl(line.text.trim_end_matches(['\n', '\r']).to_string());
                };
                let close = CancellationToken::new();
                let supervisor = tokio::spawn(supervise(child, Tree::Group, rx, pumps, cancel, close.clone()));
                return Ok(ServerHandle { url, close, supervisor: Some(supervisor) });
            }
            status = child.wait() => {
                // Collect what the pipes still hold for the error message. Bounded because a
                // surviving grandchild may keep the pipes open.
                let _ = tokio::time::timeout(Duration::from_millis(250), async {
                    while let Some(line) = rx.recv().await {
                        output.push_str(&line.text);
                    }
                })
                .await;
                let code = status.ok().and_then(|s| s.code());
                break LaunchError::Exited { code, output };
            }
            () = &mut deadline => break LaunchError::Timeout(options.timeout),
            () = cancel.cancelled() => break LaunchError::Aborted,
        }
    };
    stop(&mut child, Tree::Group);
    let _ = child.wait().await;
    for pump in pumps {
        pump.abort();
        let _ = pump.await;
    }
    Err(failure)
}

/// Running TUI child with inherited stdio.
#[derive(Debug)]
pub struct TuiHandle {
    close: CancellationToken,
    supervisor: Option<JoinHandle<Option<ExitStatus>>>,
}

impl TuiHandle {
    /// `close()` of server.ts:129-132, joined.
    pub async fn close(mut self) -> Option<ExitStatus> {
        self.close.cancel();
        self.wait_inner().await
    }

    /// Wait for the TUI to exit on its own (or via the abort token).
    pub async fn wait(mut self) -> Option<ExitStatus> {
        self.wait_inner().await
    }

    async fn wait_inner(&mut self) -> Option<ExitStatus> {
        self.supervisor.take()?.await.ok().flatten()
    }
}

impl Drop for TuiHandle {
    fn drop(&mut self) {
        self.close.cancel();
    }
}

/// Port of `createOpencodeTui` (server.ts:105-134).
pub fn create_opencode_tui(options: TuiOptions) -> Result<TuiHandle, LaunchError> {
    let mut command = Command::new(&options.program);
    [
        ("project", &options.project),
        ("model", &options.model),
        ("session", &options.session),
        ("agent", &options.agent),
    ]
    .into_iter()
    .filter_map(|(flag, value)| {
        value
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| (flag, v))
    })
    .for_each(|(flag, value)| {
        command.arg(format!("--{flag}={value}"));
    });
    command
        .env(
            "OPENCODE_CONFIG_CONTENT",
            config_content(options.config.as_ref()),
        )
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let cancel = options.cancel.unwrap_or_default();
    let child = command.spawn().map_err(LaunchError::Spawn)?;
    let close = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        child,
        Tree::Pid,
        empty_lines(),
        [],
        cancel,
        close.clone(),
    ));
    Ok(TuiHandle {
        close,
        supervisor: Some(supervisor),
    })
}

/// Own the child after launch: keep draining output, stop on abort or close, join everything.
async fn supervise<const N: usize>(
    mut child: Child,
    tree: Tree,
    mut rx: mpsc::Receiver<Line>,
    pumps: [JoinHandle<()>; N],
    cancel: CancellationToken,
    close: CancellationToken,
) -> Option<ExitStatus> {
    let status = loop {
        tokio::select! {
            Some(_) = rx.recv() => {}
            status = child.wait() => break status.ok(),
            () = cancel.cancelled() => break None,
            () = close.cancelled() => break None,
        }
    };
    let status = match status {
        Some(status) => Some(status),
        None => {
            stop(&mut child, tree);
            child.wait().await.ok()
        }
    };
    drop(rx);
    // The child has exited, a surviving descendant must not keep the supervisor alive.
    for pump in pumps {
        pump.abort();
        let _ = pump.await;
    }
    status
}

fn empty_lines() -> mpsc::Receiver<Line> {
    mpsc::channel(1).1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
struct Line {
    stream: Stream,
    /// Includes the trailing newline when present.
    text: String,
}

/// Forward lines from one pipe. Stops forwarding (but keeps reading) once the receiver is gone,
/// so the child never blocks on a full pipe.
async fn pump(pipe: Option<impl AsyncRead + Unpin>, stream: Stream, tx: mpsc::Sender<Line>) {
    let Some(pipe) = pipe else { return };
    let mut reader = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf).into_owned();
                let _ = tx.send(Line { stream, text }).await;
            }
        }
    }
}

/// `options.config?.logLevel` truthiness check of server.ts:33.
fn log_level(config: Option<&serde_json::Value>) -> Option<String> {
    match config?.get("logLevel")? {
        serde_json::Value::String(level) if !level.is_empty() => Some(level.clone()),
        _ => None,
    }
}

/// `JSON.stringify(options.config ?? {})`.
fn config_content(config: Option<&serde_json::Value>) -> String {
    config.map_or_else(|| "{}".to_string(), serde_json::Value::to_string)
}

/// First match of `/on\s+(https?:\/\/[^\s]+)/` in `line` (server.ts:53).
pub fn parse_url(line: &str) -> Option<String> {
    line.match_indices("on").find_map(|(index, _)| {
        let rest = &line[index + 2..];
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() {
            return None;
        }
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            return None;
        }
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        Some(trimmed[..end].to_string())
    })
}
