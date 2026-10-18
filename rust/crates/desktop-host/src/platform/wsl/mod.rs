//! Purpose: WSL server support: wire types shared with the renderer (`WslServersState` and
//! friends), native English messages, and the submodules for policy, runtime parsing/runner trait,
//! the real `wsl.exe` adapter, the WSL sidecar launcher and the stale-start-safe controller.
//! Scope: types only here; behavior lives in the submodules.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/exe.rs
//! |-- desktop-host/src/platform/wsl/policy.rs
//! |-- desktop-host/src/platform/wsl/runtime.rs
//! `-- desktop-host/src/platform/wsl/sidecar.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/platform/mod.rs
//! |-- desktop-host/tests/wsl_policy.rs
//! `-- desktop-host/tests/wsl_servers.rs
//! Invariants: JSON field names/casing and record key order match `packages/app/src/wsl/types.ts`.
//! Contract tests: desktop-host/tests/wsl_policy.rs, desktop-host/tests/wsl_runtime.rs, desktop-host/tests/wsl_servers.rs

pub mod controller;
pub mod exe;
pub mod policy;
pub mod runtime;
pub mod sidecar;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslRuntimeCheck {
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslInstalledDistro {
    pub name: String,
    pub version: Option<i64>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslOnlineDistro {
    pub name: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistroProbe {
    pub name: String,
    pub can_execute: bool,
    pub has_bash: bool,
    pub has_curl: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslOpencodeCheck {
    pub distro: String,
    pub resolved_path: Option<String>,
    pub version: Option<String>,
    pub expected_version: Option<String>,
    pub matches_desktop: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslServerConfig {
    pub id: String,
    pub distro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WslServerRuntime {
    Starting,
    Ready {
        url: String,
        username: Option<String>,
        password: Option<String>,
    },
    Failed {
        message: String,
    },
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslServerItem {
    pub config: WslServerConfig,
    pub runtime: WslServerRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum WslJob {
    Runtime {
        started_at: u64,
    },
    Distros {
        started_at: u64,
    },
    InstallWsl {
        started_at: u64,
    },
    InstallDistro {
        distro: String,
        started_at: u64,
    },
    ProbeAddable {
        distros: Vec<String>,
        started_at: u64,
    },
    InstallOpencode {
        distro: String,
        started_at: u64,
    },
}

/// Insertion-ordered string-keyed record, serialized as a JSON object in insertion order
/// (JS object key order for `Record<string, T>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record<T>(Vec<(String, T)>);

impl<T> Default for Record<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> Record<T> {
    pub fn get(&self, key: &str) -> Option<&T> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// JS `{ ...record, [key]: value }`: replaces in place or appends.
    pub fn insert(&mut self, key: String, value: T) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|(k, _)| k.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Serialize> Serialize for Record<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Record<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
            type Value = Record<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a record object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut record = Record::default();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    record.insert(key, value);
                }
                Ok(record)
            }
        }
        deserializer.deserialize_map(Visitor(std::marker::PhantomData))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslServersState {
    pub runtime: Option<WslRuntimeCheck>,
    pub installed: Vec<WslInstalledDistro>,
    pub online: Vec<WslOnlineDistro>,
    pub distro_probes: Record<WslDistroProbe>,
    pub opencode_checks: Record<WslOpencodeCheck>,
    pub pending_restart: bool,
    pub servers: Vec<WslServerItem>,
    pub job: Option<WslJob>,
}

/// `WslServersEvent` (`{ type: "state", state }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WslServersEvent {
    State { state: WslServersState },
}

/// English native messages (`packages/app/src/i18n/desktop-native.ts:298-318`). Localization of
/// these strings is owned by the native translation layer (see notes/connections.md).
pub mod messages {
    pub const WINDOWS_ONLY: &str = "WSL is only available on Windows";
    pub const UNAVAILABLE: &str = "WSL is unavailable";
    pub const LIST_INSTALLED: &str = "Failed to list installed WSL distros";
    pub const LIST_ONLINE: &str = "Failed to list online WSL distros";
    pub const EXECUTE_DISTRO: &str = "Cannot execute commands in distro";
    pub const INSTALL_WSL: &str = "WSL installation failed";
    pub const INSTALL_OPENCODE: &str = "OpenCode installation failed";
    pub const OPENCODE_MISSING: &str = "opencode is not installed in this distro";
    pub const OPENCODE_CANNOT_RUN: &str = "opencode is installed but could not run";
    pub const NO_VERSION: &str = "no version";
    pub const FAILED_PORT: &str = "Failed to get port";

    pub fn install_distro(distro: &str) -> String {
        format!("Failed to install distro: {distro}")
    }
    pub fn already_added(distro: &str) -> String {
        format!("{distro} is already added")
    }
    pub fn opencode_not_installed(distro: &str) -> String {
        format!("OpenCode is not installed in {distro}")
    }
    pub fn update_version(distro: &str, installed: &str, expected: &str) -> String {
        format!(
            "OpenCode update finished but {distro} still reports {installed}; expected {expected}"
        )
    }
    pub fn server_exited(code: &str, signal: &str) -> String {
        format!("WSL server exited after startup (code={code} signal={signal})")
    }
    pub fn server_exited_before_healthy(code: &str, signal: &str, output: &str) -> String {
        format!("WSL server exited before becoming healthy (code={code} signal={signal}){output}")
    }
    pub fn health_timeout(distro: &str, timeout_ms: u64) -> String {
        format!("Sidecar for {distro} health check timed out after {timeout_ms}ms")
    }
    pub fn command_timeout(command: &str, args: &str, timeout_ms: u64) -> String {
        format!("{command} {args} timed out after {timeout_ms}ms")
    }
}
