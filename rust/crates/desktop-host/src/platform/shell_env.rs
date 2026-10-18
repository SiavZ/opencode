//! Purpose: capture the user's login-shell environment (`$SHELL -il -c "env -0"`, falling back to
//! `-l`) so the sidecar sees the same PATH/toolchain env as a terminal, and merge it under explicit
//! desktop overrides.
//! Scope: unix login-shell probe with timeout. Windows skips the probe (baseline server.ts:45).
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections/sidecar.rs
//! `-- desktop-host/tests/platform.rs
//! Invariants: an interactive-probe timeout does NOT fall back to `-l` (baseline), nushell is never
//! probed, an empty or non-zero probe is `Unavailable`, the probe child is killed and reaped on timeout.
//! Contract tests: desktop-host/tests/platform.rs

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::platform::proxy::EnvMap;

/// Baseline `TIMEOUT` (`packages/desktop/src/main/shell-env.ts:5`).
pub const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Loaded(EnvMap),
    Timeout,
    Unavailable,
}

/// `resolveUserShell`: explicit `$SHELL`, then the login shell unless it is `unknown`, then `/bin/sh`.
pub fn resolve_user_shell(env_shell: Option<&str>, login_shell: Option<&str>) -> String {
    let login = login_shell.filter(|shell| !shell.is_empty() && *shell != "unknown");
    env_shell
        .filter(|shell| !shell.is_empty())
        .or(login)
        .unwrap_or("/bin/sh")
        .to_string()
}

/// `getUserShell`. Node's `userInfo().shell` reads the passwd entry; without libc (unsafe is
/// denied) the entry is read from `/etc/passwd`. macOS directory-service users are not listed there,
/// so on macOS this falls through to `$SHELL`, which launchd always sets for GUI apps.
pub fn user_shell() -> String {
    let env_shell = std::env::var("SHELL").ok();
    let login = passwd_shell();
    resolve_user_shell(env_shell.as_deref(), login.as_deref())
}

/// `parseShellEnv`: NUL-separated `KEY=value` pairs, skipping empty and `=`-leading entries.
pub fn parse_shell_env(out: &[u8]) -> EnvMap {
    String::from_utf8_lossy(out)
        .split('\0')
        .filter_map(|line| {
            let index = line.find('=')?;
            if index == 0 {
                return None;
            }
            Some((line[..index].to_string(), line[index + 1..].to_string()))
        })
        .collect()
}

pub fn is_nushell(shell: &str) -> bool {
    let raw = shell.to_lowercase();
    let name = Path::new(&raw)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    name == "nu" || name == "nu.exe" || raw.ends_with("\\nu.exe")
}

/// `mergeShellEnv`: explicit env wins over the shell snapshot.
pub fn merge_shell_env(shell: Option<&EnvMap>, env: &EnvMap) -> EnvMap {
    let mut merged = shell.cloned().unwrap_or_default();
    merged.extend(env.iter().map(|(key, value)| (key.clone(), value.clone())));
    merged
}

/// Run one probe (`-il` or `-l`) with a timeout.
pub async fn probe(shell: &str, mode: &str, timeout: Duration) -> Probe {
    let mut command = Command::new(shell);
    command
        .args([mode, "-c", "env -0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::info!("[server] Shell env probe failed for {shell} {mode}: {error}");
            return Probe::Unavailable;
        }
    };
    // `kill_on_drop` kills the probe if the timeout drops the output future; tokio reaps it.
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => return Probe::Timeout,
        Ok(Err(error)) => {
            tracing::info!("[server] Shell env probe failed for {shell} {mode}: {error}");
            return Probe::Unavailable;
        }
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        tracing::info!("[server] Shell env probe exited with non-zero status for {shell} {mode}");
        return Probe::Unavailable;
    }
    let env = parse_shell_env(&output.stdout);
    if env.is_empty() {
        tracing::info!("[server] Shell env probe returned empty env for {shell} {mode}");
        return Probe::Unavailable;
    }
    Probe::Loaded(env)
}

/// `loadShellEnv`: `-il` first; on timeout give up, otherwise fall back to `-l`.
pub async fn load_shell_env(shell: &str, timeout: Duration) -> Option<EnvMap> {
    if is_nushell(shell) {
        tracing::info!("[server] Skipping shell env probe for nushell: {shell}");
        return None;
    }
    match probe(shell, "-il", timeout).await {
        Probe::Loaded(env) => {
            tracing::info!(
                "[server] Loaded shell environment with -il ({} vars)",
                env.len()
            );
            return Some(env);
        }
        Probe::Timeout => {
            tracing::info!("[server] Interactive shell env probe timed out: {shell}");
            return None;
        }
        Probe::Unavailable => {}
    }
    if let Probe::Loaded(env) = probe(shell, "-l", timeout).await {
        tracing::info!(
            "[server] Loaded shell environment with -l ({} vars)",
            env.len()
        );
        return Some(env);
    }
    tracing::info!("[server] Falling back to app environment: {shell}");
    None
}

/// `preferAppEnv` (`packages/desktop/src/main/server.ts:44-55`) as a pure env builder: shell
/// snapshot under desktop flags, `XDG_STATE_HOME` defaulting to the user data path.
pub fn prefer_app_env(base: &EnvMap, shell_env: Option<&EnvMap>, user_data_path: &Path) -> EnvMap {
    let mut env = base.clone();
    if let Some(shell_env) = shell_env {
        env.extend(
            shell_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    env.insert("OPENCODE_EXPERIMENTAL_ICON_DISCOVERY".into(), "true".into());
    env.insert("OPENCODE_EXPERIMENTAL_FILEWATCHER".into(), "true".into());
    env.insert("OPENCODE_CLIENT".into(), "desktop".into());
    let state_home = base
        .get("XDG_STATE_HOME")
        .cloned()
        .unwrap_or_else(|| user_data_path.to_string_lossy().to_string());
    env.insert("XDG_STATE_HOME".into(), state_home);
    env
}

fn passwd_shell() -> Option<String> {
    #[cfg(unix)]
    {
        let user = std::env::var("USER").ok()?;
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        passwd
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| line.split(':').collect::<Vec<_>>())
            .find(|fields| fields.len() >= 7 && fields[0] == user)
            .map(|fields| fields[6].to_string())
    }
    #[cfg(not(unix))]
    {
        None
    }
}
