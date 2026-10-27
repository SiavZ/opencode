//! Purpose: exercise process-owned model refresh through real loopback HTTP and the public event store.
//! Scope: disabled and periodic refresh, cache identity, notifications and joined cancellation.
//! Uses:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- runtime/src/process_tasks.rs
//! `-- provider-adapters/src/catalog/refresh.rs
//! External:
//! |-- tokio::net::TcpListener
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, integration test entrypoint
//! Invariants: no external network, fake model service or detached fixture task.
//! Contract tests: runtime/tests/model_refresh.rs

use futures::StreamExt;
use opencode_event_api::{Definition, EventStore};
use opencode_runtime::{Process, ProcessOptions};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

fn inventory() -> Vec<u8> {
    serde_json::to_vec(&json!({"local":{"id":"local","name":"Local","env":[],"models":{
        "refresh-test":{"id":"refresh-test","name":"Refresh test","release_date":"2026-10-01","tool_call":true,"limit":{"context":1000,"output":100}}
    }}})).unwrap()
}

#[tokio::test]
async fn disabled_periodic_refresh_still_allows_explicit_refresh_and_publishes_real_event() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = format!("http://{}", listener.local_addr().unwrap());
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .vars
        .insert("OPENCODE_MODELS_URL".into(), source.clone());
    let process = Process::start(options).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    let mut notifications = process
        .events
        .subscribe(&Definition::ephemeral("models-dev.refreshed"));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(request.starts_with(b"GET /api.json HTTP/1.1\r\n"));
        let body = inventory();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    assert!(process.models.refresh(true).await.unwrap());
    let event = tokio::time::timeout(Duration::from_secs(5), notifications.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(event.event_type, "models-dev.refreshed");
    assert_eq!(event.data, json!({}));
    assert!(event.durable.is_none());
    assert!(
        process
            .options
            .paths
            .cache
            .join(opencode_provider_adapters::catalog::cache_filename(&source))
            .is_file()
    );
    assert_eq!(
        process
            .models
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "refresh-test"
    );
    server.await.unwrap();
    process.shutdown().await;
}

#[tokio::test]
async fn periodic_refresh_is_owned_and_shutdown_cancels_an_incomplete_http_body() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .vars
        .remove("OPENCODE_DISABLE_MODELS_FETCH");
    options.environment.vars.insert(
        "OPENCODE_MODELS_URL".into(),
        format!("http://{}", listener.local_addr().unwrap()),
    );
    let (started, received) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            request.push(socket.read_u8().await.unwrap());
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(request.starts_with(b"GET /api.json HTTP/1.1\r\n"));
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n{").await.unwrap();
        started.send(()).unwrap();
        let mut remaining = Vec::new();
        socket.read_to_end(&mut remaining).await.unwrap();
        assert!(remaining.is_empty());
    });
    let process = Process::start(options).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), process.shutdown())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
