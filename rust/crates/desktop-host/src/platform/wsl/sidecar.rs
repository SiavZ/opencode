//! Purpose: launch an opencode server inside a WSL distro (`spawnWslSidecar`): resolve the distro's
//! opencode, allocate a port and password, pipe the bootstrap script to `wsl -d <distro> -- bash -se`,
//! poll health against exit and a 30s timeout, and hand back an owned child.
//! Scope: WSL sidecar launcher. Real use requires Windows + WSL (no runner yet, see notes).
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/connections/health.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! |-- desktop-host/src/platform/process_tree.rs
//! |-- desktop-host/src/platform/proxy.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! `-- desktop-host/src/platform/wsl/runtime.rs
//! External:
//! |-- futures::FutureExt
//! |-- futures::future::BoxFuture
//! |-- messages::FAILED_PORT
//! |-- messages::health_timeout
//! |-- messages::opencode_not_installed
//! |-- messages::server_exited_before_healthy
//! |-- reqwest::Client
//! |-- tokio_util::sync::CancellationToken
//! `-- uuid::Uuid::new_v4
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! `-- desktop-host/tests/wsl_runtime.rs
//! Invariants: on any startup failure the `wsl` child tree is killed and joined before the error is
//! returned; the script exports credentials but never logs them.
//! Contract tests: desktop-host/tests/wsl_runtime.rs

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::messages;
use super::runtime::{RunOptions, WslRunner, resolve_opencode, shell_escape, wsl_args};
use crate::connections::child::{ChildServer, ChildSpec, HealthFailure, LineSink};
use crate::connections::health::BasicAuth;
use crate::connections::sidecar::allocate_port;
use crate::connections::{SERVER_USERNAME, ServerReadyData};
use crate::platform::process_tree::ExitInfo;
use crate::platform::proxy::EnvMap;

/// Handle to a running WSL sidecar process.
pub trait SidecarHandle: Send + Sync {
    /// Stop the sidecar and join its process tree.
    fn stop(&self) -> BoxFuture<'_, ()>;
    /// Resolves when the sidecar exits.
    fn exited(&self) -> BoxFuture<'_, ExitInfo>;
}

impl SidecarHandle for ChildServer {
    fn stop(&self) -> BoxFuture<'_, ()> {
        async move {
            ChildServer::stop(self).await;
        }
        .boxed()
    }
    fn exited(&self) -> BoxFuture<'_, ExitInfo> {
        ChildServer::exited(self).boxed()
    }
}

#[derive(Clone)]
pub struct RunningSidecar {
    pub handle: Arc<dyn SidecarHandle>,
    pub ready: ServerReadyData,
}

impl std::fmt::Debug for RunningSidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningSidecar")
            .field("url", &self.ready.url)
            .finish_non_exhaustive()
    }
}

pub type SpawnSidecar = Arc<
    dyn Fn(String, CancellationToken) -> BoxFuture<'static, Result<RunningSidecar, String>>
        + Send
        + Sync,
>;

/// Default WSL health timeout (`healthTimeoutMs ?? 30_000`, `wsl/sidecar.ts:65`).
pub const WSL_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Bootstrap script piped to `bash -se` (`wsl/sidecar.ts:27-39`).
pub fn bootstrap_script(opencode: &str, port: u16, password: &str, packaged: bool) -> String {
    let level = if packaged { "WARN" } else { "INFO" };
    [
        "set -euo pipefail".to_string(),
        "cd \"$HOME\" || cd /".to_string(),
        "PATH=$(awk -v RS=: -v ORS=: '$0 !~ /^\\/mnt\\//' <<<\"$PATH\" | sed \"s/:$//\")"
            .to_string(),
        "export PATH".to_string(),
        "export WSLENV=".to_string(),
        "export OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER=true".to_string(),
        "export OPENCODE_CLIENT=desktop".to_string(),
        format!(
            "export OPENCODE_SERVER_USERNAME={}",
            shell_escape(SERVER_USERNAME)
        ),
        format!("export OPENCODE_SERVER_PASSWORD={}", shell_escape(password)),
        "export XDG_STATE_HOME=\"$HOME/.local/state\"".to_string(),
        format!(
            "exec {} --print-logs --log-level {level} serve --hostname 0.0.0.0 --port {port}",
            shell_escape(opencode)
        ),
    ]
    .join("\n")
}

#[derive(Clone)]
pub struct WslSidecarOptions {
    pub packaged: bool,
    pub health_timeout: Duration,
    pub env: EnvMap,
    pub on_line: Option<LineSink>,
}

impl std::fmt::Debug for WslSidecarOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WslSidecarOptions")
            .field("packaged", &self.packaged)
            .field("health_timeout", &self.health_timeout)
            .finish_non_exhaustive()
    }
}

pub async fn spawn_wsl_sidecar(
    runner: &dyn WslRunner,
    client: &reqwest::Client,
    distro: &str,
    options: &WslSidecarOptions,
    cancel: &CancellationToken,
) -> Result<RunningSidecar, String> {
    let opencode = resolve_opencode(runner, distro, &RunOptions::with_cancel(cancel))
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| messages::opencode_not_installed(distro))?;
    let port = allocate_port("127.0.0.1")
        .await
        .map_err(|_| messages::FAILED_PORT.to_string())?;
    let password = uuid::Uuid::new_v4().to_string();
    let url = format!("http://127.0.0.1:{port}");
    let child = ChildServer::spawn(ChildSpec {
        program: "wsl".into(),
        args: wsl_args(&["bash", "-se"], Some(distro), None),
        env: options.env.clone(),
        cwd: None,
        stdin: Some(bootstrap_script(
            &opencode,
            port,
            &password,
            options.packaged,
        )),
        url: url.clone(),
        auth: BasicAuth::opencode(&password),
        stop_grace: Duration::ZERO,
        on_line: options.on_line.clone(),
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(failure) = child
        .wait_healthy(client, options.health_timeout, cancel)
        .await
    {
        child.kill().await;
        return Err(match failure {
            HealthFailure::Exited { exit, recent } => {
                let output = if recent.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", recent.join("\n"))
                };
                messages::server_exited_before_healthy(
                    &exit.code_text(),
                    &exit.signal_text(),
                    &output,
                )
            }
            HealthFailure::Timeout => {
                messages::health_timeout(distro, options.health_timeout.as_millis() as u64)
            }
            HealthFailure::Cancelled => "Aborted".to_string(),
        });
    }
    Ok(RunningSidecar {
        handle: Arc::new(child),
        ready: ServerReadyData {
            url,
            username: Some(SERVER_USERNAME.to_string()),
            password: Some(password),
        },
    })
}
