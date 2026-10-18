//! Purpose: WSL pure policy (`wsl/policy.ts`, `wsl/startup.ts`) and the renderer connection helpers
//! (`renderer/wsl/connections.ts`): restart targets, distro-state clearing, terminal args, IPC
//! argument validation, version expectation, health polling with cancellation, ready connections.
//! Also `normalizePersistedServer` and `opencodeCheck` (`wsl/servers.ts`).
//! Scope: pure functions plus one cancellable poll loop.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! `-- desktop-host/src/platform/wsl/mod.rs
//! External:
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/controller/jobs.rs
//! |-- desktop-host/tests/wsl_policy.rs
//! `-- desktop-host/tests/wsl_runtime.rs
//! Invariants: `poll_health` performs no checks after cancellation resolves.
//! Contract tests: desktop-host/tests/wsl_policy.rs, desktop-host/tests/wsl_runtime.rs

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{
    Record, WslDistroProbe, WslOpencodeCheck, WslRuntimeCheck, WslServerConfig, WslServerItem,
    WslServerRuntime, WslServersState, messages,
};
use crate::connections::{ServerConnection, ServerReadyData};

pub fn wsl_server_id_for_distro(distro: &str) -> String {
    format!("wsl:{distro}")
}

pub fn wsl_server_id_to_restart(servers: &[WslServerItem], distro: &str) -> Option<String> {
    servers
        .iter()
        .find(|item| item.config.distro == distro)
        .map(|item| item.config.id.clone())
}

pub fn clear_wsl_distro_state(
    distro_probes: &Record<WslDistroProbe>,
    opencode_checks: &Record<WslOpencodeCheck>,
    distro: &str,
) -> (Record<WslDistroProbe>, Record<WslOpencodeCheck>) {
    let mut probes = distro_probes.clone();
    let mut checks = opencode_checks.clone();
    probes.remove(distro);
    checks.remove(distro);
    (probes, checks)
}

pub fn wsl_terminal_args(distro: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["/c", "start", "", "wsl"].map(String::from).to_vec();
    if let Some(distro) = distro.filter(|distro| !distro.is_empty()) {
        args.extend(["-d".to_string(), distro.to_string()]);
    }
    args
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid {0}")]
pub struct InvalidIpcArgument(pub String);

/// `requireWslIpcString`: a non-empty JSON string.
pub fn require_ipc_string(
    name: &str,
    value: &serde_json::Value,
) -> Result<String, InvalidIpcArgument> {
    match value.as_str() {
        Some(text) if !text.is_empty() => Ok(text.to_string()),
        _ => Err(InvalidIpcArgument(name.to_string())),
    }
}

/// `requireWslIpcStrings`: a non-empty JSON array of non-empty strings.
pub fn require_ipc_strings(
    name: &str,
    value: &serde_json::Value,
) -> Result<Vec<String>, InvalidIpcArgument> {
    let items = value
        .as_array()
        .ok_or_else(|| InvalidIpcArgument(name.to_string()))?;
    let values = items
        .iter()
        .map(|item| require_ipc_string(name, item))
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        return Err(InvalidIpcArgument(name.to_string()));
    }
    Ok(values)
}

pub fn wsl_server_ids_to_start_on_initialize(servers: &[WslServerConfig]) -> Vec<String> {
    servers.iter().map(|server| server.id.clone()).collect()
}

/// `expectOpencodeVersion` (`wsl/startup.ts:7-16`).
pub fn expect_opencode_version(
    installed: Option<&str>,
    expected: &str,
    distro: &str,
) -> Result<(), String> {
    if installed == Some(expected) {
        return Ok(());
    }
    Err(messages::update_version(
        distro,
        installed.unwrap_or(messages::NO_VERSION),
        expected,
    ))
}

pub fn pending_restart_after_wsl_install(runtime: &WslRuntimeCheck) -> bool {
    !runtime.available
}

/// `pollWslHealth`: check, then wait `interval` (cut short by cancellation), until healthy or cancelled.
pub async fn poll_health<F, Fut>(mut check: F, cancel: &CancellationToken, interval: Duration)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    while !cancel.is_cancelled() {
        if check().await {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = cancel.cancelled() => {}
        }
    }
}

/// `readyWslConnections`: only `ready` servers become connections.
pub fn ready_wsl_connections(state: Option<&WslServersState>) -> Vec<ServerConnection> {
    state
        .map(|state| state.servers.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|item| match &item.runtime {
            WslServerRuntime::Ready {
                url,
                username,
                password,
            } => Some(ServerConnection::Wsl {
                distro: item.config.distro.clone(),
                ready: ServerReadyData {
                    url: url.clone(),
                    username: username.clone(),
                    password: password.clone(),
                },
            }),
            _ => None,
        })
        .collect()
}

/// `availableStartupServer`: a WSL default only wins once that server is ready.
pub fn available_startup_server(
    default_server: Option<&str>,
    state: Option<&WslServersState>,
) -> String {
    let key = default_server.unwrap_or("sidecar");
    if !key.starts_with("wsl:") {
        return key.to_string();
    }
    let ready = state.is_some_and(|state| {
        state.servers.iter().any(|item| {
            item.config.id == key && matches!(item.runtime, WslServerRuntime::Ready { .. })
        })
    });
    if ready {
        return key.to_string();
    }
    "sidecar".to_string()
}

/// State reported on non-Windows hosts (`registerUnavailableWslIpcHandlers`, `wsl/ipc.ts:72-87`).
pub fn unavailable_state() -> WslServersState {
    WslServersState {
        runtime: Some(WslRuntimeCheck {
            available: false,
            version: None,
            error: Some(messages::WINDOWS_ONLY.to_string()),
        }),
        ..WslServersState::default()
    }
}

/// `readPersistedServers`/`normalizePersistedServer` over the stored `{ servers: [...] }` record.
pub fn normalize_persisted_servers(value: &serde_json::Value) -> Vec<WslServerConfig> {
    let Some(list) = value.get("servers").and_then(|servers| servers.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            let distro = entry.get("distro")?.as_str().filter(|d| !d.is_empty())?;
            let id = entry
                .get("id")
                .and_then(|id| id.as_str())
                .filter(|id| !id.is_empty())
                .map_or_else(|| wsl_server_id_for_distro(distro), str::to_string);
            Some(WslServerConfig {
                id,
                distro: distro.to_string(),
            })
        })
        .collect()
}

/// `opencodeCheck` (`wsl/servers.ts:466-506`).
pub fn opencode_check(
    distro: &str,
    resolved: Option<String>,
    version: Option<String>,
    expected: &str,
) -> WslOpencodeCheck {
    let base = WslOpencodeCheck {
        distro: distro.to_string(),
        resolved_path: resolved.clone(),
        version: None,
        expected_version: Some(expected.to_string()),
        matches_desktop: None,
        error: None,
    };
    if resolved.is_none() {
        return WslOpencodeCheck {
            error: Some(messages::OPENCODE_MISSING.to_string()),
            ..base
        };
    }
    let Some(version) = version else {
        return WslOpencodeCheck {
            error: Some(messages::OPENCODE_CANNOT_RUN.to_string()),
            ..base
        };
    };
    WslOpencodeCheck {
        matches_desktop: Some(version == expected),
        version: Some(version),
        ..base
    }
}
