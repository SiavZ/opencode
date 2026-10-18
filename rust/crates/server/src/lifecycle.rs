//! Purpose: listener lifecycle (`packages/opencode/src/server/server.ts` listen/stop): bind the
//! socket before reporting readiness, port-zero policy (prefer 4096, then any free port), stdout
//! readiness line, and joined graceful shutdown of in-flight requests.
//! Scope: process-level listener ownership.
//! Uses:
//! rust/crates/
//! `-- server/src/app.rs
//! External:
//! |-- hyper_util::server::conn::auto
//! |-- tokio::net::TcpListener
//! `-- tokio_util::{sync::CancellationToken, task::TaskTracker}
//! Used by:
//! rust/crates/
//! `-- server/src/lib.rs
//! Invariants: `listen` returns only after `bind` succeeded, so `url()` is connectable. Explicit
//! non-zero ports never fall back. `stop(false)` stops accepting and waits for in-flight
//! requests (graceful); `stop(true)` additionally cancels them after a 1 second grace period
//! like `gracefulShutdownTimeout: "1 second"` plus `closeAllConnections`. Every spawned task is
//! tracked and joined before `stop` returns.
//! Contract tests: server/tests/http_lifecycle.rs

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Port tried first when the caller asks for port 0.
pub const PREFERRED_PORT: u16 = 4096;

/// Grace period before a forced stop drops open connections.
pub const FORCE_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenOptions {
    pub hostname: String,
    pub port: u16,
}

impl Default for ListenOptions {
    fn default() -> Self {
        ListenOptions {
            hostname: "127.0.0.1".to_string(),
            port: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[error("failed to bind {hostname}:{port}: {source}")]
    Bind {
        hostname: String,
        port: u16,
        #[source]
        source: std::io::Error,
    },
}

/// A running listener. Dropping it without `stop` cancels serving but does not join.
pub struct Listener {
    address: SocketAddr,
    hostname: String,
    shutdown: CancellationToken,
    force: CancellationToken,
    tracker: TaskTracker,
}

impl Listener {
    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// `makeURL(hostname, port)`: `http://<hostname>:<port>` (IPv6 hosts bracketed).
    pub fn url(&self) -> String {
        let host = if self.hostname.contains(':') && !self.hostname.starts_with('[') {
            format!("[{}]", self.hostname)
        } else {
            self.hostname.clone()
        };
        format!("http://{host}:{}", self.port())
    }

    /// Baseline stdout readiness line (`opencode serve`).
    pub fn readiness_line(&self) -> String {
        format!(
            "opencode server listening on http://{}:{}",
            self.hostname,
            self.port()
        )
    }

    /// Stop accepting and join every task. `close` forces open connections closed after
    /// `FORCE_GRACE`.
    pub async fn stop(self, close: bool) {
        self.shutdown.cancel();
        if close {
            let force = self.force.clone();
            let tracker = self.tracker.clone();
            tokio::select! {
                _ = tracker.wait() => {}
                _ = tokio::time::sleep(FORCE_GRACE) => force.cancel(),
            }
        }
        self.tracker.close();
        self.tracker.wait().await;
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.force.cancel();
    }
}

/// Bind and start serving `router`. Port 0 prefers 4096, then any free port.
pub async fn listen(router: Router, options: ListenOptions) -> Result<Listener, ListenError> {
    let listener = match options.port {
        0 => match bind(&options.hostname, PREFERRED_PORT).await {
            Ok(listener) => listener,
            Err(_) => bind(&options.hostname, 0).await?,
        },
        port => bind(&options.hostname, port).await?,
    };
    let address = listener.local_addr().map_err(|source| ListenError::Bind {
        hostname: options.hostname.clone(),
        port: options.port,
        source,
    })?;
    Ok(serve(listener, address, options.hostname, router))
}

/// Serve `router` on an already-bound listener (embedded hosts and tests).
pub fn serve(
    listener: TcpListener,
    address: SocketAddr,
    hostname: String,
    router: Router,
) -> Listener {
    let shutdown = CancellationToken::new();
    let force = CancellationToken::new();
    let tracker = TaskTracker::new();
    tracker.spawn(accept_loop(
        listener,
        router,
        shutdown.clone(),
        force.clone(),
        tracker.clone(),
    ));
    Listener {
        address,
        hostname,
        shutdown,
        force,
        tracker,
    }
}

/// Owned accept loop: each connection is a tracked task that drains gracefully on `shutdown`
/// and is dropped on `force`.
async fn accept_loop(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
    force: CancellationToken,
    tracker: TaskTracker,
) {
    loop {
        let (stream, _) = tokio::select! {
            _ = shutdown.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
        };
        let service = hyper_util::service::TowerToHyperService::new(router.clone());
        let shutdown = shutdown.clone();
        let force = force.clone();
        tracker.spawn(async move {
            let builder = auto::Builder::new(TokioExecutor::new());
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
            tokio::pin!(connection);
            let mut draining = false;
            loop {
                tokio::select! {
                    result = connection.as_mut() => {
                        if let Err(error) = result {
                            tracing::debug!(%error, "connection ended with error");
                        }
                        return;
                    }
                    _ = shutdown.cancelled(), if !draining => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                    _ = force.cancelled() => return,
                }
            }
        });
    }
}

async fn bind(hostname: &str, port: u16) -> Result<TcpListener, ListenError> {
    TcpListener::bind((hostname, port))
        .await
        .map_err(|source| ListenError::Bind {
            hostname: hostname.to_string(),
            port,
            source,
        })
}
