//! Purpose: the real `WslRunner` adapter: spawns `wsl.exe`/`powershell.exe`/`cmd.exe` with hidden
//! windows, enforces the per-command timeout and cancellation (kill + reap), and decodes wsl.exe's
//! UTF-16LE output (`createOutputDecoder`, `runtime.ts:183-202`).
//! Scope: process adapter. Compiles everywhere; only meaningful on Windows (no runner yet, see notes).
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/platform/wsl/runtime.rs
//! External:
//! |-- futures::future::BoxFuture
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! `-- desktop-host/tests/wsl_runtime.rs
//! Invariants: a timed-out or cancelled command is killed and reaped before the error resolves.
//! `run_interactive` uses piped stdio, not ConPTY: parity for installers that need a console is a
//! recorded blocker (notes/connections.md), not claimed.
//! Contract tests: desktop-host/tests/wsl_runtime.rs

use std::process::Stdio;

use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::process::Command;

use super::messages;
use super::runtime::{DEFAULT_WSL_TIMEOUT, RunOptions, WslCommandResult, WslError, WslRunner};

/// Real process runner for WSL commands.
#[derive(Debug, Clone, Default)]
pub struct ExeRunner {
    /// `%SystemRoot%` (or `%windir%`) used to resolve System32 commands.
    pub system_root: Option<String>,
}

impl ExeRunner {
    pub fn from_env() -> Self {
        Self {
            system_root: std::env::var("SystemRoot")
                .or_else(|_| std::env::var("windir"))
                .ok(),
        }
    }
}

impl WslRunner for ExeRunner {
    fn run(
        &self,
        program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>> {
        run_command(program.to_string(), args, options).boxed()
    }

    fn run_interactive(
        &self,
        program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>> {
        run_command(program.to_string(), args, options).boxed()
    }

    fn spawn_detached(
        &self,
        program: &str,
        args: Vec<String>,
    ) -> BoxFuture<'static, Result<(), WslError>> {
        let program = program.to_string();
        async move {
            let mut command = Command::new(&program);
            command
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            command.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | NEW_PROCESS_GROUP
            // Detached terminal window: intentionally not owned or joined (baseline `child.unref()`).
            command
                .spawn()
                .map(drop)
                .map_err(|error| WslError::Spawn(error.to_string()))
        }
        .boxed()
    }

    fn system32(&self, command: &str) -> String {
        let Some(root) = &self.system_root else {
            return command.to_string();
        };
        let resolved = std::path::Path::new(root).join("System32").join(command);
        if resolved.exists() {
            return resolved.to_string_lossy().to_string();
        }
        command.to_string()
    }
}

async fn run_command(
    program: String,
    args: Vec<String>,
    options: RunOptions,
) -> Result<WslCommandResult, WslError> {
    let timeout = options.timeout.unwrap_or(DEFAULT_WSL_TIMEOUT);
    let mut command = Command::new(&program);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW (`windowsHide: true`)
    let child = command
        .spawn()
        .map_err(|error| WslError::Spawn(error.to_string()))?;
    let cancel = options.cancel.clone().unwrap_or_default();
    // Dropping the output future drops the child, and `kill_on_drop` kills it; tokio reaps it.
    let output = tokio::select! {
        output = child.wait_with_output() => output.map_err(|error| WslError::Spawn(error.to_string()))?,
        () = tokio::time::sleep(timeout) => {
            return Err(WslError::Timeout(messages::command_timeout(
                &program,
                &args.join(" "),
                timeout.as_millis() as u64,
            )));
        }
        () = cancel.cancelled() => return Err(WslError::Aborted),
    };
    Ok(WslCommandResult {
        code: output.status.code(),
        signal: crate::platform::process_tree::ExitInfo::from_status(output.status).signal,
        stdout: decode_output(&output.stdout),
        stderr: decode_output(&output.stderr),
    })
}

/// `detectOutputEncoding` + decode: BOM `FF FE`, or mostly-zero odd bytes, means UTF-16LE.
pub fn decode_output(bytes: &[u8]) -> String {
    if is_utf16le(bytes) {
        let start = if bytes.starts_with(&[0xff, 0xfe]) {
            2
        } else {
            0
        };
        let units: Vec<u16> = bytes[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).to_string()
}

fn is_utf16le(chunk: &[u8]) -> bool {
    if chunk.len() >= 2 && chunk[0] == 0xff && chunk[1] == 0xfe {
        return true;
    }
    let pairs = chunk.len() / 2;
    if pairs < 2 {
        return false;
    }
    let odd_zeroes = (0..pairs).filter(|index| chunk[index * 2 + 1] == 0).count();
    let even_zeroes = (0..pairs).filter(|index| chunk[index * 2] == 0).count();
    odd_zeroes >= pairs.div_ceil(3) && even_zeroes * 2 <= odd_zeroes
}
