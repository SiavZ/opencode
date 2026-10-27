//! Purpose: verify hosted socket cancellation through actual TCP connections.
//! Scope: local transport serving, pending headers, HTTP keepalive and streaming bodies.
//! Uses:
//! rust/bins/
//! `-- hosted/src/transport.rs
//! External:
//! |-- axum::{Router, extract::Extension}
//! |-- tokio::net::TcpStream
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/bins/
//! `-- hosted/src/transport.rs
//! Invariants: tests never substitute oneshot requests for socket EOF; servers are aborted/joined.
//! Contract tests: this file.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Extension;
use axum::routing::{get, post};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct Server {
    address: std::net::SocketAddr,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { super::serve(listener, router).await.unwrap() });
        Self { address, task }
    }

    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

struct Dropped(Arc<Notify>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn headers(socket: &mut BufReader<TcpStream>) -> String {
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        assert!(socket.read_line(&mut line).await.unwrap() > 0);
        headers.push_str(&line);
        if line == "\r\n" {
            return headers;
        }
    }
}

#[tokio::test]
async fn tcp_disconnect_before_response_headers_cancels_and_drops_handler() {
    let dropped = Arc::new(Notify::new());
    let (tokens, mut received) = mpsc::unbounded_channel();
    let server = Server::start(Router::new().route(
        "/pending",
        get({
            let dropped = dropped.clone();
            move |Extension(token): Extension<CancellationToken>| {
                let tokens = tokens.clone();
                let dropped = dropped.clone();
                async move {
                    let _guard = Dropped(dropped);
                    tokens.send(token).unwrap();
                    std::future::pending::<&'static str>().await
                }
            }
        }),
    ))
    .await;
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket
        .write_all(b"GET /pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let token = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!token.is_cancelled());
    drop(socket);
    tokio::time::timeout(Duration::from_secs(3), token.cancelled())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), dropped.notified())
        .await
        .unwrap();
    server.stop().await;
}

#[tokio::test]
async fn completed_post_body_does_not_cancel_and_connection_remains_reusable() {
    let release = Arc::new(Notify::new());
    let (tokens, mut received) = mpsc::unbounded_channel();
    let server = Server::start(Router::new().route(
        "/body",
        post({
            let release = release.clone();
            move |Extension(token): Extension<CancellationToken>, body: Bytes| {
                let tokens = tokens.clone();
                let release = release.clone();
                async move {
                    assert_eq!(body, "data");
                    tokens.send(token.clone()).unwrap();
                    release.notified().await;
                    assert!(!token.is_cancelled());
                    "ok"
                }
            }
        }),
    ))
    .await;
    let mut socket = BufReader::new(TcpStream::connect(server.address).await.unwrap());
    let mut connection_tokens = Vec::new();
    for request in [
        &b"POST /body HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\ndata"[..],
        &b"POST /body HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata\r\n0\r\n\r\n"[..],
    ] {
        socket.get_mut().write_all(request).await.unwrap();
        let token = tokio::time::timeout(Duration::from_secs(3), received.recv())
            .await
            .unwrap()
            .unwrap();
        // A fully consumed HTTP request body is not a closed TCP connection.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), token.cancelled())
                .await
                .is_err()
        );
        release.notify_one();
        let response = tokio::time::timeout(Duration::from_secs(3), headers(&mut socket))
            .await
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        let mut body = [0; 2];
        socket.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"ok");
        connection_tokens.push(token);
    }
    assert!(connection_tokens.iter().all(|token| !token.is_cancelled()));
    drop(socket);
    for token in connection_tokens {
        tokio::time::timeout(Duration::from_secs(3), token.cancelled())
            .await
            .unwrap();
    }
    server.stop().await;
}

#[tokio::test]
async fn tcp_disconnect_drops_stream_after_response_headers() {
    let dropped = Arc::new(Notify::new());
    let (tokens, mut received) = mpsc::unbounded_channel();
    let server = Server::start(Router::new().route(
        "/stream",
        get({
            let dropped = dropped.clone();
            move |Extension(token): Extension<CancellationToken>| {
                let tokens = tokens.clone();
                let dropped = dropped.clone();
                async move {
                    tokens.send(token).unwrap();
                    Body::from_stream(futures::stream::unfold(
                        (false, Dropped(dropped)),
                        |(sent, guard)| async move {
                            if sent {
                                std::future::pending::<()>().await;
                            }
                            Some((
                                Ok::<_, Infallible>(Bytes::from_static(b"first")),
                                (true, guard),
                            ))
                        },
                    ))
                }
            }
        }),
    ))
    .await;
    let mut socket = BufReader::new(TcpStream::connect(server.address).await.unwrap());
    socket
        .get_mut()
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let token = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), headers(&mut socket))
        .await
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    let mut chunk = [0; 10];
    socket.read_exact(&mut chunk).await.unwrap();
    assert_eq!(&chunk, b"5\r\nfirst\r\n");
    assert!(!token.is_cancelled());
    drop(socket);
    tokio::time::timeout(Duration::from_secs(3), token.cancelled())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), dropped.notified())
        .await
        .unwrap();
    server.stop().await;
}

#[tokio::test]
async fn closing_one_connection_does_not_cancel_another() {
    let (tokens, mut received) = mpsc::unbounded_channel();
    let server = Server::start(Router::new().route(
        "/pending",
        get(move |Extension(token): Extension<CancellationToken>| {
            let tokens = tokens.clone();
            async move {
                tokens.send(token).unwrap();
                std::future::pending::<&'static str>().await
            }
        }),
    ))
    .await;
    let mut first = TcpStream::connect(server.address).await.unwrap();
    first
        .write_all(b"GET /pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let first_token = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    let mut second = TcpStream::connect(server.address).await.unwrap();
    second
        .write_all(b"GET /pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let second_token = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    drop(first);
    tokio::time::timeout(Duration::from_secs(3), first_token.cancelled())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), second_token.cancelled())
            .await
            .is_err()
    );
    drop(second);
    tokio::time::timeout(Duration::from_secs(3), second_token.cancelled())
        .await
        .unwrap();
    server.stop().await;
}
