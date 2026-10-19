//! Purpose: listener lifecycle over real sockets: bind before ready, explicit port, port-zero
//! fallback, readiness line, graceful and forced stop with joined tasks.
//! Scope: live local listeners.
//! Uses:
//! rust/crates/
//! |-- server/src/lifecycle.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- axum::Router
//! |-- axum::routing::get
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: `listen` returns a connectable listener; `stop` joins every task.
//! Contract tests: this file.

mod support;

use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use opencode_server::lifecycle::{ListenOptions, PREFERRED_PORT, listen};

fn slow_router() -> Router {
    Router::new()
        .route("/ok", get(|| async { "ok" }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                "slow"
            }),
        )
        .route(
            "/hang",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                "never"
            }),
        )
}

#[tokio::test]
async fn port_zero_prefers_4096_or_falls_back_and_is_connectable() {
    let server = listen(slow_router(), ListenOptions::default())
        .await
        .unwrap();
    // 4096 may be taken on a shared machine; either way the listener is bound and reachable.
    assert!(server.port() == PREFERRED_PORT || server.port() != 0);
    assert_eq!(
        server.readiness_line(),
        format!(
            "opencode server listening on http://127.0.0.1:{}",
            server.port()
        )
    );
    let body = support::client()
        .get(format!("{}/ok", server.url()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
    server.stop(false).await;
}

#[tokio::test]
async fn port_zero_falls_back_when_preferred_is_busy() {
    // Hold 4096 if free so the fallback branch runs deterministically.
    let _guard = tokio::net::TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await;
    let server = listen(slow_router(), ListenOptions::default())
        .await
        .unwrap();
    assert_ne!(server.port(), PREFERRED_PORT);
    server.stop(false).await;
}

#[tokio::test]
async fn explicit_busy_port_fails_without_fallback() {
    let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = holder.local_addr().unwrap().port();
    let result = listen(
        slow_router(),
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port,
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn graceful_stop_waits_for_in_flight_requests() {
    let server = listen(
        slow_router(),
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port: 0,
        },
    )
    .await
    .unwrap();
    let url = format!("{}/slow", server.url());
    let request = tokio::spawn(async move {
        support::client()
            .get(url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stop(false).await;
    assert_eq!(request.await.unwrap(), "slow");
}

#[tokio::test]
async fn forced_stop_drops_hung_connections_after_grace() {
    let server = listen(
        slow_router(),
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port: 0,
        },
    )
    .await
    .unwrap();
    let url = format!("{}/hang", server.url());
    let request = tokio::spawn(async move { support::client().get(url).send().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    server.stop(true).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(request.await.unwrap().is_err());
}
