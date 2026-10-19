//! Purpose: actual HTTP/SSE workspace transport and production supervisor replay/reconnect acceptance.
//! Scope: isolated Process SQLite/event store, two native loopback listeners and controlled remote frames.
//! Uses:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! |-- runtime/src/process.rs
//! |-- runtime/src/workspace_transport.rs
//! `-- server/src/lifecycle.rs
//! External:
//! |-- axum::{Router, body::Body}
//! |-- futures::{StreamExt, stream}
//! |-- serde_json::json
//! `-- tokio::sync::Notify
//! Used by:
//! `-- none, entrypoint
//! Invariants: real transport, supervisor and SQL replay; only remote adapter target/protocol peer are fixtures.
//! Contract tests: this file

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use futures::{StreamExt, stream};
use opencode_event_api::{Definition, EventStore};
use opencode_locations::workspace::{
    AdapterContext, Info, SyncTransport, Target, WorkspaceAdapter,
};
use opencode_locations::{LocationError, Result};
use opencode_runtime::process::{Process, ProcessOptions};
use opencode_runtime::workspace_transport::WorkspaceTransport;
use opencode_server::lifecycle::{ListenOptions, Listener, listen};
use opencode_storage_api::{DatabaseExt, TxOptions};
use serde_json::{Value, json};

async fn start(router: Router) -> Listener {
    listen(
        router,
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port: 0,
        },
    )
    .await
    .unwrap()
}

struct Dropped(Arc<tokio::sync::Notify>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

fn held_body(bytes: Vec<u8>, dropped: Arc<tokio::sync::Notify>) -> Body {
    Body::from_stream(stream::unfold(
        (Dropped(dropped), Some(bytes)),
        |(guard, bytes)| async move {
            if let Some(bytes) = bytes {
                return Some((Ok::<_, std::io::Error>(bytes), (guard, None)));
            }
            let _guard = guard;
            futures::future::pending::<()>().await;
            None
        },
    ))
}

#[tokio::test]
async fn transport_sends_exact_headers_fences_and_drops_owned_stream() {
    let dropped = Arc::new(tokio::sync::Notify::new());
    let signal = dropped.clone();
    let peer = start(Router::new()
        .route("/base/global/event", get(move |headers:HeaderMap| {
            assert_eq!(headers["authorization"], "Bearer fixture");
            assert_eq!(headers["accept"], "text/event-stream");
            let signal = signal.clone();
            async move { ([("content-type","text/event-stream")], held_body(b"data: {\"value\":1}\n\n".to_vec(),signal)) }
        }))
        .route("/base/sync/history", post(|headers:HeaderMap,axum::Json(body):axum::Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture");
            assert_eq!(headers["content-type"], "application/json");
            assert_eq!(body,json!({"ses_transport":7}));
            axum::Json(json!([{"id":"evt_history","aggregate_id":"ses_transport","seq":8,"type":"test.workspace.1","data":{"sessionID":"ses_transport"}}]))
        }))).await;
    let transport = WorkspaceTransport::new(reqwest::Client::builder().no_proxy().build().unwrap());
    let headers = vec![("authorization".into(), "Bearer fixture".into())];
    let history = transport
        .history(
            &opencode_locations::workspace::sync::route(
                &format!("{}/base/?discard=1", peer.url()),
                "/sync/history",
            )
            .unwrap(),
            &headers,
            &BTreeMap::from([("ses_transport".into(), 7)]),
        )
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, "evt_history");
    assert_eq!(history[0].seq, 8);
    let mut body = transport
        .connect(&format!("{}/base/global/event", peer.url()), &headers)
        .await
        .unwrap();
    assert_eq!(
        body.next().await.unwrap().unwrap(),
        b"data: {\"value\":1}\n\n"
    );
    drop(body);
    tokio::time::timeout(Duration::from_secs(3), dropped.notified())
        .await
        .unwrap();
    peer.stop(false).await;
}

#[tokio::test]
async fn non_success_and_invalid_history_keep_typed_errors_and_body() {
    let peer = start(
        Router::new()
            .route(
                "/event",
                get(|| async { (StatusCode::UNAUTHORIZED, "private failure") }),
            )
            .route(
                "/history",
                post(|| async { (StatusCode::BAD_GATEWAY, "upstream detail\n") }),
            )
            .route(
                "/invalid",
                post(|| async { ([("content-type", "application/json")], "{broken") }),
            ),
    )
    .await;
    let transport = WorkspaceTransport::new(reqwest::Client::builder().no_proxy().build().unwrap());
    let error = match transport
        .connect(&format!("{}/event", peer.url()), &[])
        .await
    {
        Ok(_) => panic!("expected HTTP failure"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        LocationError::SyncHttp {
            status: 401,
            body: None,
            ..
        }
    ));
    assert_eq!(error.to_string(), "Workspace sync HTTP failure: 401");
    let error = transport
        .history(&format!("{}/history", peer.url()), &[], &BTreeMap::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Workspace history HTTP failure: 502 upstream detail\n"
    );
    assert!(
        matches!(error,LocationError::SyncHttp {status:502,body:Some(body),..} if body=="upstream detail\n")
    );
    assert!(matches!(
        transport
            .history(&format!("{}/invalid", peer.url()), &[], &BTreeMap::new())
            .await
            .unwrap_err(),
        LocationError::Validation(_)
    ));
    assert!(matches!(
        transport
            .history(
                &format!("{}/history", peer.url()),
                &[("bad\nname".into(), "value".into())],
                &BTreeMap::new()
            )
            .await
            .unwrap_err(),
        LocationError::Validation(_)
    ));
    peer.stop(false).await;
}

struct Remote(String);
#[async_trait::async_trait]
impl WorkspaceAdapter for Remote {
    fn name(&self) -> &str {
        "Controlled loopback peer"
    }
    fn description(&self) -> &str {
        "Target-only routing fixture; provisioning is not exercised"
    }
    async fn target(&self, _: &Info, _: &AdapterContext) -> Result<Target> {
        Ok(Target::Remote {
            url: self.0.clone(),
            headers: vec![("x-fixture".into(), "workspace".into())],
        })
    }
}

const EVENT: Definition = Definition::durable("test.workspace", 1, "sessionID");

#[tokio::test]
async fn real_process_supervisor_replays_reconnects_and_shutdown_drops_stream() {
    let connections = Arc::new(AtomicUsize::new(0));
    let connect = connections.clone();
    let histories = Arc::new(AtomicUsize::new(0));
    let history = histories.clone();
    let dropped = Arc::new(tokio::sync::Notify::new());
    let signal = dropped.clone();
    let peer = start(Router::new()
        .route("/base/global/event",get(move |headers:HeaderMap| {
            assert_eq!(headers["x-fixture"],"workspace");
            assert_eq!(headers["accept"],"text/event-stream");
            let index = connect.fetch_add(1,Ordering::SeqCst);
            let signal = signal.clone();
            async move {
                let seq = if index==0 {1} else {2};
                let event = json!({"directory":"/remote","payload":{"type":"sync","syncEvent":{"id":format!("evt_transport{seq}"),"aggregateID":"ses_transport","seq":seq,"type":"test.workspace.1","data":{"sessionID":"ses_transport"}}}});
                let bytes = format!("data: {event}\n\n").into_bytes();
                ([("content-type","text/event-stream")],if index==0 {Body::from(bytes)} else {held_body(bytes,signal)})
            }
        }))
        .route("/base/sync/history",post(move |headers:HeaderMap,axum::Json(state):axum::Json<Value>| {
            assert_eq!(headers["x-fixture"],"workspace");
            let index = history.fetch_add(1,Ordering::SeqCst);
            if index==0 {assert_eq!(state,json!({}));}
            if index>0 {assert_eq!(state["ses_transport"],1);}
            async move {axum::Json(if index==0 {json!([{"id":"evt_transport0","aggregate_id":"ses_transport","seq":0,"type":"test.workspace.1","data":{"sessionID":"ses_transport"}}])} else {json!([])})}
        }))).await;
    let root = tempfile::tempdir().unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options.experimental_workspaces = true;
    let process = Process::start(options).await.unwrap();
    process.events.register([&EVENT]);
    process.workspaces.adapters().register(
        "transport-project",
        "controlled",
        Arc::new(Remote(format!("{}/base/?discard=1", peer.url()))),
    );
    process.database.transaction(TxOptions::default(),|sql| {
        sql.execute("INSERT INTO project (id,worktree,vcs,name,time_created,time_updated,sandboxes) VALUES ('transport-project','/fixture','git','transport',1,1,'[]')",&[])?;
        sql.execute("INSERT INTO workspace (id,type,name,project_id,time_used) VALUES ('wrk_transport','controlled','Remote','transport-project',0)",&[])?;
        sql.execute("INSERT INTO session (id,project_id,workspace_id,slug,directory,title,version,time_created,time_updated) VALUES ('ses_transport','transport-project','wrk_transport','slug','/fixture','transport','1',1,1)",&[])?;
        Ok::<_,opencode_storage_api::StorageError>(())
    }).await.unwrap();
    let local = process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        reqwest::get(format!("{}/global/health", local.url()))
            .await
            .unwrap()
            .status(),
        200
    );
    process
        .workspaces
        .start_workspace_syncing("transport-project", &AdapterContext::default())
        .await
        .unwrap();
    process
        .workspaces
        .wait_for_sync(
            "wrk_transport",
            &BTreeMap::from([("ses_transport".into(), 2)]),
            None,
            Some(Duration::from_secs(8)),
        )
        .await
        .unwrap();
    assert_eq!(
        process
            .events
            .latest_sequence("ses_transport")
            .await
            .unwrap(),
        2
    );
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    assert_eq!(histories.load(Ordering::SeqCst), 2);
    let owner = process
        .database
        .read(|sql| {
            let rows = sql.query(
                "SELECT owner_id FROM event_sequence WHERE aggregate_id='ses_transport'",
                &[],
            )?;
            rows[0].get::<String>("owner_id")
        })
        .await
        .unwrap();
    assert_eq!(owner, "wrk_transport");
    tokio::time::timeout(Duration::from_secs(3), process.workspaces.shutdown())
        .await
        .unwrap();
    assert!(!process.workspaces.is_syncing("wrk_transport"));
    tokio::time::timeout(Duration::from_secs(3), dropped.notified())
        .await
        .unwrap();
    local.stop(false).await;
    process.shutdown().await;
    peer.stop(false).await;
}
