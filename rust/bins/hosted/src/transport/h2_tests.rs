//! Purpose: certify HTTP/2 stream reset cancellation without closing its shared TCP connection.
//! Scope: real loopback h2c transport, pending headers and streamed response body ownership.
//! Uses:
//! rust/bins/
//! `-- hosted/src/transport.rs
//! External:
//! |-- axum::{Router, extract::Extension}
//! |-- reqwest::Client
//! |-- tokio::sync::{Notify, mpsc}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/bins/
//! `-- hosted/src/transport.rs
//! Invariants: request/body drops reset only their stream; sibling and later HTTP/2 requests live.
//! Contract tests: this file. Does not certify TLS/ALPN, reverse proxies or upgraded consumers.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, OriginalUri};
use axum::http::Version;
use axum::response::IntoResponse;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(5);

struct Dropped(Arc<Notify>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct Fixture {
    base: String,
    client: reqwest::Client,
    entered: mpsc::UnboundedReceiver<(String, CancellationToken)>,
    dropped: Arc<Notify>,
    release: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

impl Fixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (entered, received) = mpsc::unbounded_channel();
        let dropped = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let app = Router::new().route(
            "/{path}",
            get({
                let dropped = dropped.clone();
                let release = release.clone();
                move |OriginalUri(uri): OriginalUri,
                      Extension(token): Extension<CancellationToken>| {
                    let entered = entered.clone();
                    let dropped = dropped.clone();
                    let release = release.clone();
                    async move {
                        assert!(!token.is_cancelled());
                        entered.send((uri.path().to_string(), token)).unwrap();
                        match uri.path() {
                            "/pending" => {
                                let _guard = Dropped(dropped);
                                std::future::pending::<axum::response::Response>().await
                            }
                            "/sibling" => {
                                release.notified().await;
                                "sibling completed".into_response()
                            }
                            "/stream" => Body::from_stream(futures::stream::unfold(
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
                            .into_response(),
                            "/ok" => "connection reused".into_response(),
                            _ => panic!("unexpected fixture path"),
                        }
                    }
                }
            }),
        );
        let task = tokio::spawn(async move { super::serve(listener, app).await.unwrap() });
        Self {
            base,
            client: reqwest::Client::builder()
                .http2_prior_knowledge()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(DEADLINE)
                .build()
                .unwrap(),
            entered: received,
            dropped,
            release,
            task: Some(task),
        }
    }

    async fn reused(&mut self, connection: &CancellationToken) {
        let response = self
            .client
            .get(format!("{}/ok", self.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.version(), Version::HTTP_2);
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "connection reused");
        let (path, token) = tokio::time::timeout(DEADLINE, self.entered.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(path, "/ok");
        // CancellationToken equality compares Arc identity, proving accepted TCP connection reuse.
        assert_eq!(&token, connection);
        assert!(!token.is_cancelled());
    }

    async fn stop(mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[tokio::test]
async fn h2_reset_before_headers_drops_handler_and_preserves_live_connection() {
    let mut fixture = Fixture::start().await;
    let mut cancelled = Box::pin(
        fixture
            .client
            .get(format!("{}/pending", fixture.base))
            .send(),
    );
    let (path, connection) = tokio::select! {
        event = tokio::time::timeout(DEADLINE, fixture.entered.recv()) => event.unwrap().unwrap(),
        response = &mut cancelled => panic!("pending request completed before reset: {response:?}"),
    };
    assert_eq!(path, "/pending");
    let mut sibling = Box::pin(
        fixture
            .client
            .get(format!("{}/sibling", fixture.base))
            .send(),
    );
    let (path, sibling_connection) = tokio::select! {
        event = tokio::time::timeout(DEADLINE, fixture.entered.recv()) => event.unwrap().unwrap(),
        response = &mut cancelled => panic!("pending request completed before reset: {response:?}"),
        response = &mut sibling => panic!("sibling completed before release: {response:?}"),
    };
    assert_eq!(path, "/sibling");
    assert_eq!(connection, sibling_connection);
    drop(cancelled);
    // Success requires the real server handler guard to drop, never a timeout treated as success.
    tokio::time::timeout(DEADLINE, fixture.dropped.notified())
        .await
        .unwrap();
    assert!(!connection.is_cancelled());
    assert!(!sibling_connection.is_cancelled());
    fixture.release.notify_one();
    let response = sibling.await.unwrap();
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "sibling completed");
    fixture.reused(&connection).await;
    fixture.stop().await;
}

#[tokio::test]
async fn h2_reset_response_body_drops_stream_and_preserves_live_connection() {
    let mut fixture = Fixture::start().await;
    let mut response = fixture
        .client
        .get(format!("{}/stream", fixture.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.status(), 200);
    assert_eq!(
        tokio::time::timeout(DEADLINE, response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "first"
    );
    let (path, connection) = tokio::time::timeout(DEADLINE, fixture.entered.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(path, "/stream");
    let mut sibling = Box::pin(
        fixture
            .client
            .get(format!("{}/sibling", fixture.base))
            .send(),
    );
    let (path, sibling_connection) = tokio::select! {
        event = tokio::time::timeout(DEADLINE, fixture.entered.recv()) => event.unwrap().unwrap(),
        response = &mut sibling => panic!("sibling completed before release: {response:?}"),
    };
    assert_eq!(path, "/sibling");
    assert_eq!(connection, sibling_connection);
    drop(response);
    tokio::time::timeout(DEADLINE, fixture.dropped.notified())
        .await
        .unwrap();
    assert!(!connection.is_cancelled());
    assert!(!sibling_connection.is_cancelled());
    fixture.release.notify_one();
    let response = sibling.await.unwrap();
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "sibling completed");
    fixture.reused(&connection).await;
    fixture.stop().await;
}
