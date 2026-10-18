//! Purpose: WSL runtime operations over a pluggable command runner: `wsl` argument building,
//! command output parsing (installed/online distro lists, first line, summarize), shell escaping,
//! runtime/distro/opencode probes and installs (`packages/desktop/src/main/wsl/runtime.ts`).
//! Scope: logic over the `WslRunner` trait; the real process runner lives in exe.rs.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/platform/wsl/mod.rs
//! External:
//! |-- futures::future::BoxFuture
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/exe.rs
//! |-- desktop-host/src/platform/wsl/sidecar.rs
//! `-- desktop-host/tests/wsl_runtime.rs
//! Invariants: every runner call carries a timeout (20s default, 15min for installs) and the job's
//! cancellation token; probe errors become typed results, list errors propagate.
//! Contract tests: desktop-host/tests/wsl_runtime.rs

use std::time::Duration;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::{WslDistroProbe, WslInstalledDistro, WslOnlineDistro, WslRuntimeCheck, messages};

/// `DEFAULT_WSL_TIMEOUT_MS` (`runtime.ts:34`).
pub const DEFAULT_WSL_TIMEOUT: Duration = Duration::from_secs(20);
/// `DEFAULT_WSL_INSTALL_TIMEOUT_MS` (`runtime.ts:35`).
pub const DEFAULT_WSL_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslCommandResult {
    pub code: Option<i32>,
    pub signal: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WslError {
    /// The job was cancelled (`AbortError`); jobs treat this as a silent no-op.
    #[error("Aborted")]
    Aborted,
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Spawn(String),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub cancel: Option<CancellationToken>,
    pub timeout: Option<Duration>,
}

impl RunOptions {
    pub fn with_cancel(cancel: &CancellationToken) -> Self {
        Self {
            cancel: Some(cancel.clone()),
            timeout: None,
        }
    }

    fn or_timeout(&self, timeout: Duration) -> Self {
        Self {
            cancel: self.cancel.clone(),
            timeout: Some(self.timeout.unwrap_or(timeout)),
        }
    }
}

/// Executes host commands for WSL operations. `run` pipes stdout/stderr; `run_interactive` needs a
/// console (ConPTY in the baseline) for installers that refuse to run without a terminal.
pub trait WslRunner: Send + Sync {
    fn run(
        &self,
        program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>>;

    fn run_interactive(
        &self,
        program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>>;

    /// Launch a detached process (terminal window) and resolve once it spawned.
    fn spawn_detached(
        &self,
        program: &str,
        args: Vec<String>,
    ) -> BoxFuture<'static, Result<(), WslError>>;

    /// Resolve `%SystemRoot%\System32\<command>` when it exists.
    fn system32(&self, command: &str) -> String {
        command.to_string()
    }
}

pub fn wsl_args(args: &[&str], distro: Option<&str>, user: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(distro) = distro.filter(|value| !value.is_empty()) {
        out.extend(["-d".to_string(), distro.to_string()]);
    }
    if let Some(user) = user.filter(|value| !value.is_empty()) {
        out.extend(["--user".to_string(), user.to_string()]);
    }
    out.push("--".to_string());
    out.extend(args.iter().map(|arg| arg.to_string()));
    out
}

pub fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn summarize(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn first_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// `parseInstalledDistros`: `^\s*(\*)?\s*(.*?)\s{2,}\S+\s+(\d+)\s*$` per line, header skipped.
pub fn parse_installed_distros(output: &str) -> Vec<WslInstalledDistro> {
    output.lines().filter_map(parse_installed_line).collect()
}

fn parse_installed_line(line: &str) -> Option<WslInstalledDistro> {
    let body = line.trim_end();
    if body.trim().is_empty() {
        return None;
    }
    let version_start = body.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let version = &body[version_start..];
    let before_version = &body[..version_start];
    if version.is_empty() || before_version.len() == before_version.trim_end().len() {
        return None;
    }
    let before_version = before_version.trim_end();
    let state_start = before_version
        .rfind(char::is_whitespace)
        .map(|index| index + 1)?;
    let before_state = &before_version[..state_start];
    let name_part = before_state.trim_end();
    let gap = before_state[name_part.len()..].chars().count();
    if gap < 2 {
        return None;
    }
    let lead = name_part.trim_start();
    let (marker, name) = match lead.strip_prefix('*') {
        Some(rest) => (true, rest.trim()),
        None => (false, lead.trim()),
    };
    if name.is_empty() || name.eq_ignore_ascii_case("name") {
        return None;
    }
    Some(WslInstalledDistro {
        name: name.to_string(),
        version: version.parse().ok(),
        is_default: marker,
    })
}

/// `parseOnlineDistros`: `^([A-Za-z0-9._-]+)\s{2,}(.+)$` on trimmed lines, header skipped.
pub fn parse_online_distros(output: &str) -> Vec<WslOnlineDistro> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let name_end = trimmed
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
                .unwrap_or(trimmed.len());
            let name = &trimmed[..name_end];
            let rest = &trimmed[name_end..];
            let label = rest.trim_start();
            let gap = rest[..rest.len() - label.len()].chars().count();
            if name.is_empty() || gap < 2 || label.is_empty() || name.eq_ignore_ascii_case("name") {
                return None;
            }
            Some(WslOnlineDistro {
                name: name.to_string(),
                label: label.trim().to_string(),
            })
        })
        .collect()
}

pub async fn run_wsl(
    runner: &dyn WslRunner,
    args: Vec<String>,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    runner.run("wsl", args, options.clone()).await
}

pub async fn run_in_distro(
    runner: &dyn WslRunner,
    args: &[&str],
    distro: Option<&str>,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    run_wsl(runner, wsl_args(args, distro, None), options).await
}

pub async fn run_sh(
    runner: &dyn WslRunner,
    script: &str,
    distro: Option<&str>,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    run_in_distro(runner, &["sh", "-lc", script], distro, options).await
}

fn failed(error: WslError) -> WslCommandResult {
    WslCommandResult {
        code: Some(1),
        signal: None,
        stdout: String::new(),
        stderr: error.to_string(),
    }
}

fn message_or(result: &WslCommandResult, fallback: &str) -> String {
    let text = if result.stderr.is_empty() {
        &result.stdout
    } else {
        &result.stderr
    };
    let summary = summarize(text);
    if summary.is_empty() {
        return fallback.to_string();
    }
    summary
}

pub async fn probe_runtime(runner: &dyn WslRunner, options: &RunOptions) -> WslRuntimeCheck {
    let version = run_wsl(runner, vec!["--version".into()], options)
        .await
        .unwrap_or_else(failed);
    if version.code != Some(0) {
        return WslRuntimeCheck {
            available: false,
            version: None,
            error: Some(message_or(&version, messages::UNAVAILABLE)),
        };
    }
    WslRuntimeCheck {
        available: true,
        version: first_line(&version.stdout),
        error: None,
    }
}

pub async fn list_installed(
    runner: &dyn WslRunner,
    options: &RunOptions,
) -> Result<Vec<WslInstalledDistro>, WslError> {
    let result = run_wsl(runner, vec!["--list".into(), "--verbose".into()], options).await?;
    if result.code != Some(0) {
        return Err(WslError::Message(message_or(
            &result,
            messages::LIST_INSTALLED,
        )));
    }
    Ok(parse_installed_distros(&result.stdout))
}

pub async fn list_online(
    runner: &dyn WslRunner,
    options: &RunOptions,
) -> Result<Vec<WslOnlineDistro>, WslError> {
    let result = run_wsl(runner, vec!["--list".into(), "--online".into()], options).await?;
    if result.code != Some(0) {
        return Err(WslError::Message(message_or(
            &result,
            messages::LIST_ONLINE,
        )));
    }
    Ok(parse_online_distros(&result.stdout))
}

pub async fn install_wsl_runtime_elevated(
    runner: &dyn WslRunner,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    let script = [
        "$ErrorActionPreference = 'Stop'",
        "$process = Start-Process -FilePath 'wsl.exe' -Verb RunAs -ArgumentList @('--install','--no-distribution') -Wait -PassThru",
        "if ($null -ne $process.ExitCode) { exit $process.ExitCode }",
    ]
    .join("; ");
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]
    .map(String::from)
    .to_vec();
    runner
        .run(
            "powershell.exe",
            args,
            options.or_timeout(DEFAULT_WSL_INSTALL_TIMEOUT),
        )
        .await
}

pub async fn install_distro(
    runner: &dyn WslRunner,
    name: &str,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    let args = ["--install", "-d", name, "--web-download", "--no-launch"]
        .map(String::from)
        .to_vec();
    runner
        .run_interactive(
            &runner.system32("wsl.exe"),
            args,
            options.or_timeout(DEFAULT_WSL_INSTALL_TIMEOUT),
        )
        .await
}

pub fn install_opencode_script(version: &str) -> String {
    format!(
        "curl -fsSL https://opencode.ai/install | bash -s -- --version {}",
        shell_escape(version)
    )
}

pub async fn install_opencode(
    runner: &dyn WslRunner,
    version: &str,
    distro: &str,
    options: &RunOptions,
) -> Result<WslCommandResult, WslError> {
    let script = install_opencode_script(version);
    let args = wsl_args(&["bash", "-lc", &script], Some(distro), None);
    runner
        .run_interactive(
            &runner.system32("wsl.exe"),
            args,
            options.or_timeout(DEFAULT_WSL_INSTALL_TIMEOUT),
        )
        .await
}

pub async fn probe_distro(
    runner: &dyn WslRunner,
    name: &str,
    options: &RunOptions,
) -> Result<WslDistroProbe, WslError> {
    let executable = run_in_distro(runner, &["/bin/true"], Some(name), options)
        .await
        .unwrap_or_else(failed);
    if executable.code != Some(0) {
        return Ok(WslDistroProbe {
            name: name.to_string(),
            can_execute: false,
            has_bash: false,
            has_curl: false,
            error: Some(message_or(&executable, messages::EXECUTE_DISTRO)),
        });
    }
    let (bash, curl) = futures::future::join(
        run_sh(
            runner,
            "command -v bash >/dev/null && printf yes || printf no",
            Some(name),
            options,
        ),
        run_sh(
            runner,
            "command -v curl >/dev/null && printf yes || printf no",
            Some(name),
            options,
        ),
    )
    .await;
    let (bash, curl) = (bash?, curl?);
    Ok(WslDistroProbe {
        name: name.to_string(),
        can_execute: true,
        has_bash: bash.code == Some(0) && summarize(&bash.stdout) == "yes",
        has_curl: curl.code == Some(0) && summarize(&curl.stdout) == "yes",
        error: None,
    })
}

pub async fn resolve_opencode(
    runner: &dyn WslRunner,
    distro: &str,
    options: &RunOptions,
) -> Result<Option<String>, WslError> {
    let result = run_sh(
        runner,
        "if [ -x \"$HOME/.opencode/bin/opencode\" ]; then printf \"%s\\n\" \"$HOME/.opencode/bin/opencode\"; fi",
        Some(distro),
        options,
    )
    .await?;
    Ok(first_line(&result.stdout))
}

pub async fn read_command_version(
    runner: &dyn WslRunner,
    command: &str,
    distro: &str,
    options: &RunOptions,
) -> Result<Option<String>, WslError> {
    let script = format!("{} --version 2>/dev/null || true", shell_escape(command));
    let result = run_sh(runner, &script, Some(distro), options).await?;
    Ok(first_line(&result.stdout))
}

pub async fn open_terminal(runner: &dyn WslRunner, distro: Option<&str>) -> Result<(), WslError> {
    runner
        .spawn_detached("cmd.exe", super::policy::wsl_terminal_args(distro))
        .await
}
