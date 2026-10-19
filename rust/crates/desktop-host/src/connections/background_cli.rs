//! Purpose: v2 shared background daemon (`startBackgroundCli`): stage the bundled CLI into a
//! versioned user-data directory (atomic temp+rename), discover an already-running service across
//! candidate state homes, start it, and read its password. The daemon is shared and detached: the
//! host never kills it.
//! Scope: CLI subprocess orchestration with env-scoped XDG_STATE_HOME; secrets redacted from logs.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! |-- futures::future::join_all
//! |-- tokio::fs::copy
//! |-- tokio::fs::create_dir_all
//! |-- tokio::fs::remove_file
//! |-- tokio::fs::rename
//! |-- tokio::fs::set_permissions
//! |-- tokio::fs::try_exists
//! |-- tokio::process::Command
//! |-- tracing::error
//! |-- tracing::info
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! `-- desktop-host/tests/connections.rs
//! Invariants: the returned connection is `ServerConnection::Daemon`; staged binaries are reused when
//! present; a failed rename removes the temp file; password output is never logged.
//! Known discrepancy (Phase 90 blocker): the desktop runs `service get password` while the CLI spec
//! declares `service password [value]`. `PasswordCommand` makes the choice explicit; the default is
//! the desktop baseline. See rust/contracts/phase-11/notes/connections.md.
//! Contract tests: desktop-host/tests/connections.rs

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use super::{ConnectionError, SERVER_USERNAME, ServerConnection, ServerReadyData};
use crate::platform::proxy::EnvMap;

/// Desktop app-data directory names scanned for an existing daemon (`background-cli.ts:11`).
pub const DESKTOP_STATE_NAMES: [&str; 3] = [
    "ai.opencode.desktop.dev",
    "ai.opencode.desktop.beta",
    "ai.opencode.desktop",
];

/// Which CLI vocabulary reads the daemon password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasswordCommand {
    /// `service get password`, what the desktop baseline runs (`background-cli.ts:43`).
    #[default]
    DesktopBaseline,
    /// `service password`, what the CLI spec declares (`packages/cli/src/commands/commands.ts:36-39`).
    CliSpec,
}

impl PasswordCommand {
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Self::DesktopBaseline => &["service", "get", "password"],
            Self::CliSpec => &["service", "password"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundCliConfig {
    /// Bundled executable (resources/opencode-cli[.exe]).
    pub bundled: PathBuf,
    /// Stage into `<user_data>/cli/<version>/` when packaged; run the bundled binary otherwise.
    pub packaged: bool,
    pub user_data: PathBuf,
    pub app_data: PathBuf,
    /// Process env (`XDG_STATE_HOME` in it is the host's own state home).
    pub env: EnvMap,
    /// `XDG_STATE_HOME` reported by the login shell probe.
    pub shell_state_home: Option<String>,
    pub password_command: PasswordCommand,
}

/// `executableName()` (`background-cli.ts:123-125`).
pub fn executable_name(windows: bool) -> &'static str {
    if windows {
        return "opencode-cli.exe";
    }
    "opencode-cli"
}

/// `serviceUrl` (`background-cli.ts:110-115`): a bare URL, or `running <url>`.
pub fn service_url(status: &str) -> Option<String> {
    if url::Url::parse(status).is_ok() {
        return Some(status.to_string());
    }
    let rest = status.strip_prefix("running ")?.trim();
    url::Url::parse(rest).ok().map(|_| rest.to_string())
}

/// Version string sanitized into a directory name (`background-cli.ts:62`).
pub fn version_dir(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                return c;
            }
            '-'
        })
        .collect()
}

/// Ordered unique candidate state homes: host env, shell env, then each desktop app-data dir; only
/// `None` (meaning "unset XDG_STATE_HOME") or existing paths survive (`background-cli.ts:27-29`).
pub fn state_home_candidates(
    state_home: Option<&str>,
    shell_state_home: Option<&str>,
    app_data: &Path,
) -> Vec<Option<String>> {
    let mut all: Vec<Option<String>> = vec![
        state_home.map(str::to_string),
        shell_state_home.map(str::to_string),
    ];
    all.extend(
        DESKTOP_STATE_NAMES
            .iter()
            .map(|name| Some(app_data.join(name).to_string_lossy().to_string())),
    );
    let mut unique: Vec<Option<String>> = Vec::new();
    for candidate in all {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    unique
        .into_iter()
        .filter(|candidate| {
            candidate
                .as_ref()
                .is_none_or(|path| Path::new(path).exists())
        })
        .collect()
}

/// Copy the bundled CLI into `<user_data>/cli/<version>/` unless already staged (`installCli`).
pub async fn stage_cli(
    source: &Path,
    user_data: &Path,
    version: &str,
    windows: bool,
) -> Result<PathBuf, ConnectionError> {
    let directory = user_data.join("cli").join(version_dir(version));
    let destination = directory.join(executable_name(windows));
    if tokio::fs::try_exists(&destination).await.unwrap_or(false) {
        tracing::info!(path = %destination.display(), version, "v2 CLI staged executable reused");
        return Ok(destination);
    }
    let temp = PathBuf::from(format!(
        "{}.{}.tmp",
        destination.display(),
        std::process::id()
    ));
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| io_error(&directory, error))?;
    tokio::fs::copy(source, &temp)
        .await
        .map_err(|error| io_error(&temp, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|error| io_error(&temp, error))?;
    }
    if let Err(error) = tokio::fs::rename(&temp, &destination).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(io_error(&destination, error));
    }
    tracing::info!(source = %source.display(), path = %destination.display(), version, "v2 CLI executable staged");
    Ok(destination)
}

/// Run one CLI command with `XDG_STATE_HOME` set to `state_home` (removed when `None`), returning
/// trimmed stdout. Non-zero exit is a typed error (`run`, `background-cli.ts:79-108`).
pub async fn run_cli(
    binary: &Path,
    args: &[&str],
    env: &EnvMap,
    state_home: Option<&str>,
    redact: bool,
) -> Result<String, ConnectionError> {
    let mut env = env.clone();
    match state_home {
        Some(home) => env.insert("XDG_STATE_HOME".into(), home.to_string()),
        None => env.remove("XDG_STATE_HOME"),
    };
    tracing::info!(binary = %binary.display(), ?args, "v2 CLI command started");
    let mut command = Command::new(binary);
    command
        .args(args)
        .env_clear()
        .envs(&env)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .await
        .map_err(|error| ConnectionError::Command {
            command: format!("{} {}", binary.display(), args.join(" ")),
            message: error.to_string(),
            stderr: String::new(),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let shown = if redact {
        "[redacted]"
    } else {
        stdout.as_str()
    };
    if !output.status.success() {
        tracing::error!(?args, stdout = shown, stderr, "v2 CLI command failed");
        return Err(ConnectionError::Command {
            command: format!("{} {}", binary.display(), args.join(" ")),
            message: format!("exited with code {:?}", output.status.code()),
            stderr,
        });
    }
    tracing::info!(?args, stdout = shown, stderr, "v2 CLI command completed");
    Ok(stdout)
}

/// `startBackgroundCli`: resolve/stage the binary, discover a running daemon in parallel across
/// candidate state homes, `service start` in the discovered (or host) state home, read the password.
pub async fn start_background_cli(
    config: &BackgroundCliConfig,
) -> Result<ServerConnection, ConnectionError> {
    // Baseline `run(bundled, ["--version"])` passes no stateHome, which unsets XDG_STATE_HOME.
    let version = run_cli(&config.bundled, &["--version"], &config.env, None, false).await?;
    let binary = if config.packaged {
        stage_cli(&config.bundled, &config.user_data, &version, cfg!(windows)).await?
    } else {
        config.bundled.clone()
    };
    let state_home = config.env.get("XDG_STATE_HOME").cloned();
    let candidates = state_home_candidates(
        state_home.as_deref(),
        config.shell_state_home.as_deref(),
        &config.app_data,
    );
    let statuses = futures::future::join_all(candidates.iter().map(|candidate| {
        run_cli(
            &binary,
            &["service", "status"],
            &config.env,
            candidate.as_deref(),
            false,
        )
    }))
    .await;
    // Baseline uses Promise.all: one failing status command fails startup.
    let mut found = None;
    for (candidate, status) in candidates.iter().zip(statuses) {
        if found.is_none() && service_url(&status?).is_some() {
            found = Some(candidate.clone());
        }
    }
    let daemon_state_home = found.clone().unwrap_or(state_home);
    let url = run_cli(
        &binary,
        &["service", "start"],
        &config.env,
        daemon_state_home.as_deref(),
        false,
    )
    .await?;
    let password = run_cli(
        &binary,
        config.password_command.args(),
        &config.env,
        daemon_state_home.as_deref(),
        true,
    )
    .await?;
    tracing::info!(
        existing = found.is_some(),
        url,
        "v2 CLI background service ready"
    );
    Ok(ServerConnection::Daemon(ServerReadyData {
        url,
        username: Some(SERVER_USERNAME.to_string()),
        password: Some(password),
    }))
}

fn io_error(path: &Path, error: std::io::Error) -> ConnectionError {
    ConnectionError::Io {
        path: path.to_string_lossy().to_string(),
        message: error.to_string(),
    }
}
