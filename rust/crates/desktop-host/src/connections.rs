//! Purpose: server connection identity and ownership for the desktop host: owned local sidecar
//! child, shared detached background daemon, remote HTTP server with basic auth, and WSL sidecars
//! owned by the WSL controller. Only owned connections are stopped on quit/kill-sidecar.
//! Scope: connection enum, ready data, the process-wide sidecar slot (`killSidecar`) and errors.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections/background_cli.rs
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/connections/health.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! `-- desktop-host/src/platform/process_tree.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- tokio::sync::Mutex
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections/background_cli.rs
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/connections/health.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! |-- desktop-host/src/lib.rs
//! |-- desktop-host/src/platform/wsl/policy.rs
//! |-- desktop-host/src/platform/wsl/sidecar.rs
//! |-- desktop-host/tests/connections.rs
//! |-- desktop-host/tests/connections_live.rs
//! |-- desktop-host/tests/wsl_policy.rs
//! `-- desktop-host/tests/wsl_support/mod.rs
//! Invariants: `SidecarSlot::kill_sidecar` stops an `OwnedSidecar` only (joined process-tree stop) and
//! leaves daemon, remote and WSL connections running; the slot is cleared before the stop awaits.
//! Contract tests: desktop-host/tests/connections.rs, desktop-host/tests/connections_live.rs

pub mod background_cli;
pub mod child;
pub mod health;
pub mod sidecar;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::platform::process_tree::ExitInfo;

pub use child::{ChildServer, ChildSpec, HealthFailure, OutputLine, OutputStream};
pub use health::{BasicAuth, check_health};

/// Username the desktop host uses for every server it launches (`server.ts:67`, `sidecar.ts:56`).
pub const SERVER_USERNAME: &str = "opencode";

/// `ServerReadyData` (`packages/desktop/src/preload/types.ts:19-23`), the payload handed to the
/// renderer once a connection is usable. Wire names match the TypeScript shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerReadyData {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// A server the desktop app talks to, tagged with who owns its process.
#[derive(Debug, Clone)]
pub enum ServerConnection {
    /// Local sidecar the host spawned (v1 `spawnLocalServer`). Killed on quit/kill-sidecar.
    OwnedSidecar(Arc<ChildServer>, ServerReadyData),
    /// Shared detached background service (v2 `startBackgroundCli`). Never killed by the host.
    Daemon(ServerReadyData),
    /// User-configured remote HTTP server with optional basic auth. Never killed by the host.
    Remote(ServerReadyData),
    /// WSL sidecar; its child is owned by `WslServersController` (stopped by its `stop_all`).
    Wsl {
        distro: String,
        ready: ServerReadyData,
    },
}

impl ServerConnection {
    pub fn ready_data(&self) -> &ServerReadyData {
        match self {
            Self::OwnedSidecar(_, ready) | Self::Daemon(ready) | Self::Remote(ready) => ready,
            Self::Wsl { ready, .. } => ready,
        }
    }

    /// Renderer connection key (`packages/app/src/context/server.tsx:220-231`): builtin sidecar
    /// (owned or daemon) is `sidecar`, WSL is `wsl:<distro>`, remote HTTP is its URL.
    pub fn key(&self) -> String {
        match self {
            Self::OwnedSidecar(..) | Self::Daemon(_) => "sidecar".to_string(),
            Self::Wsl { distro, .. } => format!("wsl:{distro}"),
            Self::Remote(ready) => ready.url.clone(),
        }
    }

    /// Whether quitting the host must stop this connection's process tree directly.
    pub fn owned_by_host(&self) -> bool {
        matches!(self, Self::OwnedSidecar(..))
    }

    pub fn basic_auth(&self) -> Option<BasicAuth> {
        let ready = self.ready_data();
        let password = ready.password.clone()?;
        Some(BasicAuth {
            username: ready
                .username
                .clone()
                .unwrap_or_else(|| SERVER_USERNAME.to_string()),
            password,
        })
    }
}

/// Errors surfaced by connection startup and control. Startup failures resolve every readiness
/// waiter with one of these (`forwardInitializationFailure`, `initialization.ts`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConnectionError {
    #[error("failed to spawn {program}: {message}")]
    Spawn { program: String, message: String },
    #[error("Failed to get port")]
    Port(String),
    #[error("Sidecar did not become ready within {timeout_ms}ms: {program}")]
    ReadyTimeout { timeout_ms: u64, program: String },
    #[error("Sidecar exited before health check passed with code {}", .0.code_text())]
    ExitedBeforeHealthy(ExitInfo),
    #[error("sidecar health check timed out after {timeout_ms}ms")]
    HealthTimeout { timeout_ms: u64 },
    #[error("{command} failed: {message}")]
    Command {
        command: String,
        message: String,
        stderr: String,
    },
    #[error("io error at {path}: {message}")]
    Io { path: String, message: String },
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("cancelled")]
    Cancelled,
}

/// The `server` variable plus `killSidecar` (`packages/desktop/src/main/index.ts:67,88-93`).
#[derive(Debug, Default)]
pub struct SidecarSlot {
    current: tokio::sync::Mutex<Option<ServerConnection>>,
}

impl SidecarSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, connection: ServerConnection) {
        *self.current.lock().await = Some(connection);
    }

    pub async fn current(&self) -> Option<ServerConnection> {
        self.current.lock().await.clone()
    }

    /// Clears the slot first, then stops an owned sidecar and joins its process tree. Daemon,
    /// remote and WSL connections are dropped from the slot but their processes keep running.
    pub async fn kill_sidecar(&self) -> Option<ExitInfo> {
        let taken = self.current.lock().await.take()?;
        match taken {
            ServerConnection::OwnedSidecar(child, _) => child.stop().await,
            _ => None,
        }
    }
}
