//! Purpose: launch the owned local sidecar server (`spawnLocalServer`): port allocation
//! (`OPENCODE_PORT` or an ephemeral loopback port), random password, sidecar env (credentials,
//! state home, loopback NO_PROXY, DEBUG/LD_PRELOAD stripped), readiness via health polling with a
//! stall timeout, startup error propagation and kill-on-failure.
//! Scope: v1 desktop sidecar. The v2 shared daemon lives in background_cli.rs.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/connections/health.rs
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! |-- tokio::net::TcpListener
//! |-- tokio_util::sync::CancellationToken
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- desktop-host/tests/connections.rs
//! `-- desktop-host/tests/connections_live.rs
//! Invariants: a sidecar that fails, exits, times out or is cancelled before healthy is stopped and
//! reaped before the error is returned; the returned connection is always `OwnedSidecar`.
//! Contract tests: desktop-host/tests/connections.rs, desktop-host/tests/connections_live.rs

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::child::{ChildServer, ChildSpec, HealthFailure, LineSink};
use super::health::BasicAuth;
use super::{ConnectionError, SERVER_USERNAME, ServerConnection, ServerReadyData};
use crate::platform::proxy::{EnvMap, ensure_loopback_no_proxy};

/// `SIDECAR_START_STALL_TIMEOUT` (`packages/desktop/src/main/server.ts:20`).
pub const SIDECAR_START_STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// `SIDECAR_STOP_TIMEOUT` (`server.ts:21`).
pub const SIDECAR_STOP_TIMEOUT: Duration = Duration::from_secs(6);
/// Loopback hostname the desktop binds its sidecar to (`index.ts:370`).
pub const SIDECAR_HOSTNAME: &str = "127.0.0.1";
/// CORS origin the renderer uses (`sidecar.ts:62`).
pub const RENDERER_ORIGIN: &str = "oc://renderer";

#[derive(Clone)]
pub struct SidecarConfig {
    /// Server executable (the Rust `opencode` binary, or a test server).
    pub program: PathBuf,
    /// Full argument list; use [`serve_args`] for the opencode server.
    pub args: Vec<String>,
    pub hostname: String,
    pub port: u16,
    pub password: String,
    pub user_data_path: PathBuf,
    /// Environment the host prepared (`preferAppEnv` output); further sanitized by [`sidecar_env`].
    pub base_env: EnvMap,
    pub cwd: Option<PathBuf>,
    pub ready_timeout: Duration,
    pub stop_grace: Duration,
    pub on_line: Option<LineSink>,
}

impl std::fmt::Debug for SidecarConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SidecarConfig")
            .field("program", &self.program)
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// `opencode serve` arguments equivalent to `Server.listen({ port, hostname, cors })`.
pub fn serve_args(hostname: &str, port: u16) -> Vec<String> {
    [
        "serve",
        "--hostname",
        hostname,
        "--port",
        &port.to_string(),
        "--cors",
        RENDERER_ORIGIN,
    ]
    .iter()
    .map(|value| value.to_string())
    .collect()
}

/// New sidecar password (`randomUUID()`, `index.ts:372`).
pub fn new_password() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `OPENCODE_PORT` override with `Number.parseInt` semantics: leading whitespace, optional sign,
/// leading digits; anything else (or NaN) means "allocate a port". Out-of-range values are errors.
pub fn port_from_env(env: &EnvMap) -> Result<Option<u16>, ConnectionError> {
    let Some(raw) = env.get("OPENCODE_PORT").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let trimmed = raw.trim_start();
    let (sign, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return Ok(None);
    }
    let value = digits.parse::<i64>().map(|value| value * sign).ok();
    match value.and_then(|value| u16::try_from(value).ok()) {
        Some(port) => Ok(Some(port)),
        None => Err(ConnectionError::Port(format!(
            "invalid OPENCODE_PORT {raw}"
        ))),
    }
}

/// Ephemeral loopback port: bind `hostname:0`, read the port, close (`index.ts:349-366`).
pub async fn allocate_port(hostname: &str) -> Result<u16, ConnectionError> {
    let listener = tokio::net::TcpListener::bind((hostname, 0))
        .await
        .map_err(|error| ConnectionError::Port(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| ConnectionError::Port(error.to_string()))?
        .port();
    drop(listener);
    Ok(port)
}

/// Env for the sidecar child: `createSidecarEnv` + `prepareSidecarEnv` + `ensureLoopbackNoProxy`
/// (`server.ts:211-218`, `sidecar.ts:75-101`) plus the embedded web UI opt-out (`index.ts:123`).
pub fn sidecar_env(base: &EnvMap, password: &str, user_data_path: &Path, linux: bool) -> EnvMap {
    let mut env = base.clone();
    env.remove("DEBUG");
    if linux {
        env.remove("LD_PRELOAD");
    }
    env.insert(
        "OPENCODE_SERVER_USERNAME".into(),
        SERVER_USERNAME.to_string(),
    );
    env.insert("OPENCODE_SERVER_PASSWORD".into(), password.to_string());
    env.entry("XDG_STATE_HOME".into())
        .or_insert_with(|| user_data_path.to_string_lossy().to_string());
    env.insert("OPENCODE_DISABLE_EMBEDDED_WEB_UI".into(), "true".into());
    ensure_loopback_no_proxy(&mut env);
    env
}

/// Spawn the owned sidecar and resolve once it is healthy. Startup failures stop the child (joined)
/// and propagate as typed errors (`server.ts:96-140`).
pub async fn spawn_local_sidecar(
    config: SidecarConfig,
    client: &reqwest::Client,
    cancel: &CancellationToken,
) -> Result<ServerConnection, ConnectionError> {
    let url = format!("http://{}:{}", config.hostname, config.port);
    let env = sidecar_env(
        &config.base_env,
        &config.password,
        &config.user_data_path,
        cfg!(target_os = "linux"),
    );
    let child = ChildServer::spawn(ChildSpec {
        program: config.program.clone(),
        args: config.args.clone(),
        env,
        cwd: config.cwd.clone(),
        stdin: None,
        url: url.clone(),
        auth: BasicAuth::opencode(&config.password),
        stop_grace: config.stop_grace,
        on_line: config.on_line.clone(),
    })
    .await?;
    if let Err(failure) = child
        .wait_healthy(client, config.ready_timeout, cancel)
        .await
    {
        child.kill().await;
        return Err(match failure {
            HealthFailure::Exited { exit, .. } => ConnectionError::ExitedBeforeHealthy(exit),
            HealthFailure::Timeout => ConnectionError::ReadyTimeout {
                timeout_ms: config.ready_timeout.as_millis() as u64,
                program: child.program().to_string(),
            },
            HealthFailure::Cancelled => ConnectionError::Cancelled,
        });
    }
    Ok(ServerConnection::OwnedSidecar(
        Arc::new(child),
        ServerReadyData {
            url,
            username: Some(SERVER_USERNAME.to_string()),
            password: Some(config.password),
        },
    ))
}
