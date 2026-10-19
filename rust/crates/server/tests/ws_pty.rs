//! Purpose: fixture socket tests for PTY WebSocket connect: replay chunks, cursor meta frame and
//! live ordering, input forwarding, cursor query, ticket single use/scope/origin, 404 before
//! upgrade, exited close codes, abrupt disconnect detaches, shutdown closes 1001 and joins.
//! Scope: live local listener with an in-memory `PtyService` (fixture, not adapter).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/services/mod.rs
//! |-- server/src/services/streams.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- app::router
//! |-- async_trait::async_trait
//! |-- futures_util::SinkExt
//! |-- futures_util::StreamExt
//! |-- mpsc::UnboundedReceiver
//! |-- mpsc::UnboundedSender
//! |-- mpsc::unbounded_channel
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::Mutex::new
//! |-- tokio::sync::mpsc
//! |-- tokio::test
//! |-- tokio::time::sleep
//! |-- tokio::time::timeout
//! |-- tokio_tungstenite::connect_async
//! |-- tokio_tungstenite::tungstenite::Message
//! `-- tokio_tungstenite::tungstenite::client::IntoClientRequest
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-based; the native PTY backend is tested in platform-native.
//! Contract tests: this file.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use opencode_platform_api::pty::{
    PtyAttachment, PtyCreateInput, PtyError, PtyInfo, PtyOutput, PtyService, PtyStatus,
    PtyUpdateInput, ReplayFrom,
};
use opencode_server::app::{self, ServerConfig};
use opencode_server::middleware::cors::CorsConfig;
use opencode_server::services::PortError;
use opencode_server::services::streams::{PtyProvider, StreamServices, TicketScope, TicketStore};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Debug)]
struct FakeAttachment {
    replay: String,
    cursor: u64,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<PtyOutput>>,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl PtyAttachment for FakeAttachment {
    fn replay(&self) -> &str {
        &self.replay
    }
    fn cursor(&self) -> u64 {
        self.cursor
    }
    fn write(&self, data: &str) -> Result<(), PtyError> {
        self.log.lock().unwrap().push(format!("write:{data}"));
        Ok(())
    }
    fn activate(&self) {
        self.log.lock().unwrap().push("activate".into());
    }
    async fn next(&self) -> Option<PtyOutput> {
        self.rx.lock().await.recv().await
    }
    fn detach(&self) {
        self.log.lock().unwrap().push("detach".into());
    }
}

#[derive(Debug, Default)]
struct FakePty {
    log: Arc<Mutex<Vec<String>>>,
    live: Mutex<Option<mpsc::UnboundedSender<PtyOutput>>>,
    exited: bool,
    from: Mutex<Vec<ReplayFrom>>,
}

fn info(id: &str, status: PtyStatus) -> PtyInfo {
    PtyInfo {
        id: id.into(),
        title: "t".into(),
        command: "sh".into(),
        args: vec![],
        cwd: "/work".into(),
        status,
        pid: 1,
        exit_code: None,
    }
}

#[async_trait]
impl PtyService for FakePty {
    async fn list(&self) -> Vec<PtyInfo> {
        vec![]
    }
    async fn get(&self, id: &str) -> Result<PtyInfo, PtyError> {
        if id != "pty_1" {
            return Err(PtyError::NotFound(id.into()));
        }
        Ok(info(
            id,
            if self.exited {
                PtyStatus::Exited
            } else {
                PtyStatus::Running
            },
        ))
    }
    async fn create(&self, _: PtyCreateInput) -> Result<PtyInfo, PtyError> {
        Err(PtyError::NotFound("x".into()))
    }
    async fn update(&self, id: &str, _: PtyUpdateInput) -> Result<PtyInfo, PtyError> {
        Err(PtyError::NotFound(id.into()))
    }
    async fn remove(&self, id: &str) -> Result<(), PtyError> {
        Err(PtyError::NotFound(id.into()))
    }
    async fn write(&self, _: &str, _: &str) -> Result<(), PtyError> {
        Ok(())
    }
    async fn attach(&self, id: &str, from: ReplayFrom) -> Result<Box<dyn PtyAttachment>, PtyError> {
        self.from.lock().unwrap().push(from);
        if self.exited {
            return Err(PtyError::Exited(id.into()));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        *self.live.lock().unwrap() = Some(tx);
        Ok(Box::new(FakeAttachment {
            replay: "hello ".into(),
            cursor: 6,
            rx: tokio::sync::Mutex::new(rx),
            log: self.log.clone(),
        }))
    }
    async fn shutdown(&self) {}
}

struct Provider(Arc<FakePty>);

#[async_trait]
impl PtyProvider for Provider {
    async fn pty(&self, _: &str, _: Option<&str>) -> Result<Arc<dyn PtyService>, PortError> {
        Ok(self.0.clone())
    }
}

struct Harness {
    server: opencode_server::lifecycle::Listener,
    pty: Arc<FakePty>,
    tickets: Arc<TicketStore>,
    sockets: Arc<opencode_server::services::streams::SocketTracker>,
}

async fn harness(exited: bool) -> Harness {
    let pty = Arc::new(FakePty {
        exited,
        ..FakePty::default()
    });
    let tickets = Arc::new(TicketStore::default());
    let mut services = support::services(Arc::new(support::FakeSessions::default()));
    services.streams = StreamServices {
        pty: Some(Arc::new(Provider(pty.clone()))),
        tickets: tickets.clone(),
        ..StreamServices::default()
    };
    let sockets = services.streams.sockets.clone();
    let config = ServerConfig {
        full_host: true,
        cors: Some(CorsConfig::default()),
        ..ServerConfig::default()
    };
    let server = support::start(app::router(services, config)).await;
    Harness {
        server,
        pty,
        tickets,
        sockets,
    }
}

fn ws_url(server: &opencode_server::lifecycle::Listener, path: &str) -> String {
    format!("ws://127.0.0.1:{}{}", server.port(), path)
}

#[tokio::test]
async fn replay_then_cursor_then_live_then_close() {
    let h = harness(false).await;
    let url = ws_url(
        &h.server,
        "/api/pty/pty_1/connect?location%5Bdirectory%5D=/work",
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap(),
        Message::Text("hello ".into())
    );
    let meta = ws.next().await.unwrap().unwrap();
    assert_eq!(meta.into_data().to_vec(), b"\0{\"cursor\":6}".to_vec());
    let live = h.pty.live.lock().unwrap().clone().unwrap();
    live.send(PtyOutput::Data("world".into())).unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap(),
        Message::Text("world".into())
    );
    ws.send(Message::Text("ls\n".into())).await.unwrap();
    ws.send(Message::Binary(vec![0xff, 0xfe].into()))
        .await
        .unwrap();
    ws.send(Message::Binary(b"pwd\n".to_vec().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    live.send(PtyOutput::End { exit_code: Some(0) }).unwrap();
    match ws.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1000),
        other => panic!("expected close, got {other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let log = h.pty.log.lock().unwrap().clone();
    assert_eq!(log, vec!["activate", "write:ls\n", "write:pwd\n", "detach"]);
    h.server.stop(true).await;
}

#[tokio::test]
async fn cursor_query_maps_to_replay_from() {
    let h = harness(false).await;
    for (query, expected) in [
        ("", ReplayFrom::Start),
        ("&cursor=-1", ReplayFrom::End),
        ("&cursor=12", ReplayFrom::Cursor(12)),
        ("&cursor=bad", ReplayFrom::Start),
    ] {
        let url = ws_url(
            &h.server,
            &format!("/pty/pty_1/connect?directory=/work{query}"),
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws.next().await.unwrap().unwrap();
        assert_eq!(*h.pty.from.lock().unwrap().last().unwrap(), expected);
        ws.close(None).await.ok();
    }
    h.server.stop(true).await;
}

#[tokio::test]
async fn missing_pty_is_404_before_upgrade() {
    let h = harness(false).await;
    let response = support::client()
        .get(format!("{}/api/pty/pty_nope/connect", h.server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "");
    h.server.stop(true).await;
}

#[tokio::test]
async fn ticket_is_single_use_scoped_and_origin_checked() {
    let h = harness(false).await;
    let scope = TicketScope {
        pty_id: "pty_1".into(),
        directory: Some("/work".into()),
        workspace_id: None,
    };
    let ticket =
        opencode_server::services::streams::PtyTickets::issue(&*h.tickets, scope.clone()).ticket;
    let path = format!("/api/pty/pty_1/connect?location%5Bdirectory%5D=/work&ticket={ticket}");

    // Disallowed origin: 403 and the ticket survives (origin is checked first).
    let mut request = ws_url(&h.server, &path).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "https://evil.com".parse().unwrap());
    let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(error.to_string().contains("403"), "{error}");

    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(&h.server, &path))
        .await
        .unwrap();
    ws.next().await.unwrap().unwrap();
    ws.close(None).await.ok();

    let reuse = tokio_tungstenite::connect_async(ws_url(&h.server, &path))
        .await
        .unwrap_err();
    assert!(reuse.to_string().contains("403"), "{reuse}");

    let other = opencode_server::services::streams::PtyTickets::issue(
        &*h.tickets,
        TicketScope {
            directory: Some("/elsewhere".into()),
            ..scope
        },
    )
    .ticket;
    let wrong = format!("/api/pty/pty_1/connect?location%5Bdirectory%5D=/work&ticket={other}");
    let error = tokio_tungstenite::connect_async(ws_url(&h.server, &wrong))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("403"), "{error}");
    h.server.stop(true).await;
}

#[tokio::test]
async fn exited_pty_closes_with_4404() {
    let h = harness(true).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(
        &h.server,
        "/api/pty/pty_1/connect?location%5Bdirectory%5D=/work",
    ))
    .await
    .unwrap();
    match ws.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4404);
            assert_eq!(frame.reason.as_str(), "session exited");
        }
        other => panic!("expected close, got {other:?}"),
    }
    // Legacy treats an exited PTY as missing before upgrade.
    let legacy =
        tokio_tungstenite::connect_async(ws_url(&h.server, "/pty/pty_1/connect?directory=/work"))
            .await
            .unwrap_err();
    assert!(legacy.to_string().contains("404"), "{legacy}");
    h.server.stop(true).await;
}

#[tokio::test]
async fn abrupt_disconnect_detaches() {
    let h = harness(false).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(
        &h.server,
        "/api/pty/pty_1/connect?location%5Bdirectory%5D=/work",
    ))
    .await
    .unwrap();
    ws.next().await.unwrap().unwrap();
    drop(ws);
    for _ in 0..50 {
        if h.pty.log.lock().unwrap().contains(&"detach".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.pty.log.lock().unwrap().contains(&"detach".to_string()));
    h.server.stop(true).await;
}

#[tokio::test]
async fn shutdown_closes_1001_and_joins_sockets() {
    let h = harness(false).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(
        &h.server,
        "/api/pty/pty_1/connect?location%5Bdirectory%5D=/work",
    ))
    .await
    .unwrap();
    ws.next().await.unwrap().unwrap();
    ws.next().await.unwrap().unwrap();
    assert_eq!(h.sockets.active(), 1);
    tokio::time::timeout(Duration::from_secs(5), h.sockets.close_all())
        .await
        .unwrap();
    assert_eq!(h.sockets.active(), 0);
    match ws.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 1001);
            assert_eq!(frame.reason.as_str(), "server closing");
        }
        other => panic!("expected close, got {other:?}"),
    }
    // New sockets after closing are refused with 1001.
    let (mut late, _) = tokio_tungstenite::connect_async(ws_url(
        &h.server,
        "/api/pty/pty_1/connect?location%5Bdirectory%5D=/work",
    ))
    .await
    .unwrap();
    match late.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1001),
        other => panic!("expected close, got {other:?}"),
    }
    h.server.stop(true).await;
}
