//! Purpose: Socket Mode transport fixtures for ACK, dedup expiry/eviction, overload,
//! reconnect, heartbeat failure, cancellation and typed Web API failures/retries.
//! Scope: loopback axum Web API and websocket, fixture tokens only.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/slack/api.rs
//! |-- ecosystem/src/slack/mod.rs
//! |-- ecosystem/src/slack/socket.rs
//! `-- ecosystem/tests/slack_support.rs
//! External:
//! |-- serde_json::json
//! |-- tokio::sync::mpsc
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: every socket transport and fixture server is stopped and joined.
//! Contract tests: this file.

mod slack_support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use opencode_ecosystem::slack::SlackError;
use opencode_ecosystem::slack::socket::{self, SocketConfig};
use serde_json::json;
use slack_support::{Server, envelope, wait_for};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn config() -> SocketConfig {
    SocketConfig {
        reconnect_base: Duration::from_millis(5),
        reconnect_max: Duration::from_millis(20),
        ..Default::default()
    }
}

#[tokio::test]
async fn fixture_malformed_json_and_frame_limit_recovery() {
    let server = Server::start().await;
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(4);
    let config = SocketConfig {
        max_frame_bytes: 512,
        ..config()
    };
    let task = tokio::spawn(socket::run(server.api.clone(), config, tx, cancel.clone()));
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) == 1).await;
    server.send_raw("{not-json".into()).await;
    server.send(envelope("before", "C", "T", "text")).await;
    assert_eq!(rx.recv().await.unwrap().envelope_id, "before");
    server.send_raw("x".repeat(513)).await;
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) == 2).await;
    server.send(envelope("after", "C", "T", "text")).await;
    assert_eq!(rx.recv().await.unwrap().envelope_id, "after");
    wait_for(|| server.logs.acks.lock().unwrap().len() == 2).await;
    cancel.cancel();
    task.await.unwrap().unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_envelope_dedup_is_bounded_and_expires() {
    let server = Server::start().await;
    let (tx, mut rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let config = SocketConfig {
        dedup_capacity: 2,
        dedup_ttl: Duration::from_millis(300),
        ..config()
    };
    let task = tokio::spawn(socket::run(server.api.clone(), config, tx, cancel.clone()));
    for id in ["a", "b", "c", "a"] {
        server.send(envelope(id, "C", "1", "text")).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
                .envelope_id,
            id
        );
    }
    // Capacity eviction let the fourth `a` through. Its next redelivery is suppressed.
    server.send(envelope("a", "C", "1", "text")).await;
    wait_for(|| server.logs.acks.lock().unwrap().len() == 5).await;
    assert!(rx.try_recv().is_err());
    tokio::time::sleep(Duration::from_millis(350)).await;
    let mut retry = envelope("a", "C", "1", "text");
    retry["retry_attempt"] = json!(2);
    retry["retry_reason"] = json!("timeout");
    server.send(retry).await;
    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.retry_attempt, Some(2));
    assert_eq!(event.retry_reason.as_deref(), Some("timeout"));
    cancel.cancel();
    task.await.unwrap().unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_queue_saturation_is_unacked_and_typed() {
    let server = Server::start().await;
    let (tx, rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(socket::run(server.api.clone(), config(), tx, cancel));
    server.send(envelope("first", "C", "1", "text")).await;
    wait_for(|| rx.len() == 1 && server.logs.acks.lock().unwrap().len() == 1).await;
    server.send(envelope("second", "C", "1", "text")).await;
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(SlackError::QueueFull)));
    assert_eq!(
        *server.logs.acks.lock().unwrap(),
        vec![json!({"envelope_id":"first","payload":{}})]
    );
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_socket_close_reopens_connection_and_keeps_dedup() {
    let server = Server::start().await;
    let (tx, mut rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(socket::run(
        server.api.clone(),
        config(),
        tx,
        cancel.clone(),
    ));
    server.send(envelope("a", "C", "1", "text")).await;
    assert_eq!(rx.recv().await.unwrap().envelope_id, "a");
    wait_for(|| server.logs.acks.lock().unwrap().len() == 1).await;
    server.close_socket().await;
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) == 2).await;
    server.send(envelope("a", "C", "1", "text")).await;
    server.send(envelope("b", "C", "1", "text")).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .envelope_id,
        "b"
    );
    wait_for(|| server.logs.acks.lock().unwrap().len() == 3).await;
    assert!(rx.try_recv().is_err());
    cancel.cancel();
    task.await.unwrap().unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_heartbeat_timeout_reconnects_and_shutdown_interrupts_backoff() {
    let server = Server::start().await;
    server.logs.silent.store(true, Ordering::SeqCst);
    let (tx, _rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let config = SocketConfig {
        heartbeat_interval: Duration::from_millis(10),
        heartbeat_timeout: Duration::from_millis(35),
        ..config()
    };
    let task = tokio::spawn(socket::run(server.api.clone(), config, tx, cancel.clone()));
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) >= 2).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.shutdown().await;

    let server = Server::start().await;
    server.logs.http_status.store(503, Ordering::SeqCst);
    let (tx, _rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let config = SocketConfig {
        reconnect_base: Duration::from_secs(60),
        reconnect_max: Duration::from_secs(60),
        ..SocketConfig::default()
    };
    let task = tokio::spawn(socket::run(
        server.api.clone().with_retries(0),
        config,
        tx,
        cancel.clone(),
    ));
    wait_for(|| !server.logs.calls.lock().unwrap().is_empty()).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_slack_api_errors_and_rate_limit_retry() {
    let server = Server::start().await;
    let api = server.api.clone().with_retries(0);
    server.logs.http_status.store(429, Ordering::SeqCst);
    assert!(matches!(
        api.post_message("C", None, "text").await,
        Err(SlackError::RateLimited { retry_after: 0 })
    ));
    server.logs.http_status.store(500, Ordering::SeqCst);
    assert!(matches!(
        api.post_message("C", None, "text").await,
        Err(SlackError::Http { status: 500 })
    ));
    server.logs.http_status.store(0, Ordering::SeqCst);
    *server.logs.api_error.lock().unwrap() = Some("not_in_channel".into());
    assert!(
        matches!(api.post_message("C", None, "text").await, Err(SlackError::Api { code }) if code == "not_in_channel")
    );
    *server.logs.api_error.lock().unwrap() = None;
    server.logs.http_status.store(429, Ordering::SeqCst);
    server.logs.retry_count.store(1, Ordering::SeqCst);
    server
        .api
        .post_message("C", Some("1"), "retried")
        .await
        .unwrap();
    assert_eq!(
        *server.logs.posts.lock().unwrap(),
        vec![json!({"channel":"C","text":"retried","thread_ts":"1"})]
    );
    assert!(!format!("{api:?}").contains("fixture"));
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_invalid_socket_limits_fail_before_network() {
    let server = Server::start().await;
    let (tx, _rx) = mpsc::channel(1);
    let config = SocketConfig {
        dedup_capacity: 0,
        ..config()
    };
    assert!(matches!(
        socket::run(server.api.clone(), config, tx, CancellationToken::new()).await,
        Err(SlackError::Protocol(_))
    ));
    assert!(server.logs.calls.lock().unwrap().is_empty());
    server.shutdown().await;
}
