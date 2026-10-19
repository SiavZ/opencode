//! Purpose: real loopback remote-workspace WebSocket relay acceptance and close/cancellation edges.
//! Scope: two listeners and an actual tungstenite client; workspace routing is fixture-supplied.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/services/mod.rs
//! |-- server/src/services/streams.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- axum::{Router, extract::ws}
//! |-- futures_util::{SinkExt, StreamExt}
//! |-- tokio::time
//! `-- tokio_tungstenite
//! Used by:
//! `-- none, entrypoint
//! Invariants: assertions observe real frames, target URL/header handling and joined shutdown.
//! Contract tests: this file

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use opencode_server::app::{ServerConfig, router};
use opencode_server::services::streams::SocketTracker;
use opencode_server::services::{LegacyWorkspaces, Services, WorkspaceInfo, WorkspaceTarget};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Routing(String);

#[async_trait::async_trait]
impl LegacyWorkspaces for Routing {
    fn configured_workspace_id(&self) -> Option<String> {
        None
    }
    async fn session(&self, _: &str) -> Option<(String, Option<String>)> {
        None
    }
    async fn workspace(&self, id: &str) -> Option<WorkspaceInfo> {
        (id == "wrk_remote").then(|| WorkspaceInfo { id: id.into() })
    }
    async fn target(&self, _: &WorkspaceInfo) -> WorkspaceTarget {
        WorkspaceTarget::Remote {
            url: self.0.clone(),
            headers: vec![("authorization".into(), "not-forwarded".into())],
        }
    }
    async fn is_syncing(&self, _: &str) -> bool {
        true
    }
}

async fn proxy(target: String) -> (opencode_server::lifecycle::Listener, Arc<SocketTracker>) {
    let services = Services {
        workspaces: Some(Arc::new(Routing(target))),
        ..Services::default()
    };
    let sockets = services.streams.sockets.clone();
    (
        support::start(router(services, ServerConfig::default())).await,
        sockets,
    )
}

#[tokio::test]
async fn relay_preserves_text_binary_close_and_sanitizes_target() {
    let (seen, observed) = tokio::sync::oneshot::channel();
    let seen = Arc::new(std::sync::Mutex::new(Some(seen)));
    let upstream = support::start(Router::new().route(
        "/base/pty/{id}/connect",
        get(
            move |upgrade: WebSocketUpgrade, uri: Uri, headers: HeaderMap| {
                seen.lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send((uri, headers))
                    .unwrap();
                async move {
                    upgrade
                        .protocols(["relay-v1"])
                        .on_upgrade(|mut socket| async move {
                            while let Some(Ok(frame)) = socket.recv().await {
                                if frame == Message::Text("close".into()) {
                                    socket
                                        .send(Message::Close(Some(CloseFrame {
                                            code: 4401,
                                            reason: "remote finished".into(),
                                        })))
                                        .await
                                        .unwrap();
                                    return;
                                }
                                let close = matches!(frame, Message::Close(_));
                                if socket.send(frame).await.is_err() || close {
                                    return;
                                }
                            }
                        })
                }
            },
        ),
    ))
    .await;
    let (server, sockets) = proxy(format!("{}/base", upstream.url())).await;
    let mut request = format!(
        "{}/pty/pty_remote/connect?workspace=wrk_remote&directory=/private&cursor=7",
        server.url().replacen("http", "ws", 1)
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "relay-v1".parse().unwrap());
    request
        .headers_mut()
        .insert("x-opencode-directory", "/private".parse().unwrap());
    request
        .headers_mut()
        .insert("authorization", "Bearer private".parse().unwrap());
    let (mut client, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status(), 101);
    let (uri, headers) = tokio::time::timeout(Duration::from_secs(3), observed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(uri.to_string(), "/base/pty/pty_remote/connect?cursor=7");
    assert_eq!(headers["sec-websocket-protocol"], "relay-v1");
    assert!(!headers.contains_key("authorization"));
    assert!(!headers.contains_key("x-opencode-directory"));
    client
        .send(tungstenite::Message::Text("hello".into()))
        .await
        .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        tungstenite::Message::Text("hello".into())
    );
    client
        .send(tungstenite::Message::Binary(vec![0, 1, 255].into()))
        .await
        .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        tungstenite::Message::Binary(vec![0, 1, 255].into())
    );
    client
        .send(tungstenite::Message::Text("close".into()))
        .await
        .unwrap();
    let tungstenite::Message::Close(Some(frame)) = client.next().await.unwrap().unwrap() else {
        panic!("expected close")
    };
    assert_eq!(u16::from(frame.code), 4401);
    assert_eq!(frame.reason, "remote finished");
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), sockets.close_all())
        .await
        .unwrap();
    assert_eq!(sockets.active(), 0);
    server.stop(false).await;
    upstream.stop(false).await;
}

#[tokio::test]
async fn shutdown_closes_both_real_peers_and_joins_relay() {
    let (closed, observe_closed) = tokio::sync::oneshot::channel();
    let closed = Arc::new(std::sync::Mutex::new(Some(closed)));
    let upstream = support::start(Router::new().route(
        "/pty/{id}/connect",
        get(move |upgrade: WebSocketUpgrade| {
            let closed = closed.lock().unwrap().take().unwrap();
            async move {
                upgrade.on_upgrade(|mut socket| async move {
                    socket.send(Message::Text("ready".into())).await.unwrap();
                    while let Some(Ok(frame)) = socket.recv().await {
                        if let Message::Close(frame) = frame {
                            closed
                                .send(frame.map(|frame| (frame.code, frame.reason.to_string())))
                                .unwrap();
                            return;
                        }
                    }
                })
            }
        }),
    ))
    .await;
    let (server, sockets) = proxy(upstream.url()).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!(
        "{}/pty/pty_remote/connect?workspace=wrk_remote",
        server.url().replacen("http", "ws", 1)
    ))
    .await
    .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        tungstenite::Message::Text("ready".into())
    );
    tokio::time::timeout(Duration::from_secs(3), sockets.close_all())
        .await
        .unwrap();
    let tungstenite::Message::Close(Some(frame)) = client.next().await.unwrap().unwrap() else {
        panic!("expected close")
    };
    assert_eq!(u16::from(frame.code), 1001);
    assert_eq!(frame.reason, "server closing");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), observe_closed)
            .await
            .unwrap()
            .unwrap(),
        Some((1001, "server closing".into()))
    );
    assert_eq!(sockets.active(), 0);
    drop(client);
    server.stop(false).await;
    upstream.stop(false).await;
}

#[tokio::test]
async fn failed_upstream_closes_accepted_socket_with_proxy_error() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let (server, sockets) = proxy(format!("http://{address}")).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!(
        "{}/pty/pty_remote/connect?workspace=wrk_remote",
        server.url().replacen("http", "ws", 1)
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tungstenite::Message::Close(Some(frame)) = frame else {
        panic!("expected close")
    };
    assert_eq!(u16::from(frame.code), 1011);
    assert_eq!(frame.reason, "proxy error");
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), sockets.close_all())
        .await
        .unwrap();
    server.stop(false).await;
}
