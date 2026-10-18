//! Purpose: remote workspace sync tests: history replay through `replay_all` and live `sync` events
//! through `replay` with workspace ownership, bus re-emission and heartbeat filtering, `waitForSync`
//! fences (resolve, timeout, abort) and the `parseSSE` decoder.
//! Scope: live tests (migrated in-memory SQLite, real `SqlEventStore`) with a scripted transport
//! standing in for the Phase 06 HTTP/SSE client.
//! Uses:
//! rust/crates/
//! |-- locations/src/lib.rs
//! `-- locations/tests/support/mod.rs
//! External:
//! |-- opencode_event_store::SqlEventStore
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the baseline behavior it pins (control-plane/workspace.ts).
//! Contract tests: locations/tests/workspace_sync.rs

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use opencode_event_api::{Definition, EventStore};
use opencode_locations::workspace::{
    AdapterContext, AdapterRegistry, CreateEnv, CreateInput, HistoryEvent, Info, SseDecoder,
    SyncTransport, Target, UnsupportedTransport, WorkspaceAdapter,
};
use opencode_locations::{LocationError, Result};
use serde_json::json;
use support::workspace::*;
use support::*;

const PROMPTED: Definition = Definition::durable("test.prompted", 1, "sessionID");

/// Scripted remote: history once, then an SSE body, then the connection ends.
struct Remote {
    history: Vec<HistoryEvent>,
    sse: Vec<u8>,
    states: Mutex<Vec<BTreeMap<String, i64>>>,
    urls: Mutex<Vec<String>>,
}

#[async_trait]
impl SyncTransport for Remote {
    async fn connect(
        &self,
        url: &str,
        _: &[(String, String)],
    ) -> Result<BoxStream<'static, Result<Vec<u8>>>> {
        self.urls.lock().unwrap().push(url.to_owned());
        let body = self.sse.clone();
        Ok(Box::pin(futures::stream::iter(vec![Ok(body)])))
    }
    async fn history(
        &self,
        url: &str,
        _: &[(String, String)],
        state: &BTreeMap<String, i64>,
    ) -> Result<Vec<HistoryEvent>> {
        self.urls.lock().unwrap().push(url.to_owned());
        self.states.lock().unwrap().push(state.clone());
        Ok(self.history.clone())
    }
}

struct RemoteAdapter;

#[async_trait]
impl WorkspaceAdapter for RemoteAdapter {
    fn name(&self) -> &str {
        "Remote"
    }
    fn description(&self) -> &str {
        ""
    }
    async fn configure(&self, info: Info, _: &AdapterContext) -> Result<Info> {
        Ok(info)
    }
    async fn create(
        &self,
        _: &Info,
        _: &CreateEnv,
        _: Option<&Info>,
        _: &AdapterContext,
    ) -> Result<()> {
        Ok(())
    }
    async fn target(&self, _: &Info, _: &AdapterContext) -> Result<Target> {
        Ok(Target::Remote {
            url: "http://remote.test/base/?q=1".into(),
            headers: vec![],
        })
    }
}

fn event(id: &str, aggregate: &str, seq: i64) -> serde_json::Value {
    json!({ "id": id, "aggregate_id": aggregate, "seq": seq, "type": "test.prompted.1", "data": { "sessionID": aggregate } })
}

#[tokio::test]
async fn remote_sync_replays_history_and_live_events_then_wait_for_sync_resolves() {
    let db = db().await;
    let history: Vec<HistoryEvent> = [
        event("evt_a0", "ses_a", 0),
        event("evt_a1", "ses_a", 1),
        event("evt_b0", "ses_b", 0),
    ]
    .into_iter()
    .map(|e| serde_json::from_value(e).unwrap())
    .collect();
    let live = json!({ "payload": { "type": "sync", "syncEvent": { "id": "evt_a2", "type": "test.prompted.1", "seq": 2, "aggregateID": "ses_a", "data": { "sessionID": "ses_a" } } } });
    let other =
        json!({ "directory": "/remote", "payload": { "type": "custom.thing", "properties": {} } });
    let heartbeat = json!({ "payload": { "type": "server.heartbeat" } });
    let sse = format!("data: {heartbeat}\n\ndata: {live}\n\ndata: {other}\n\n").into_bytes();
    let remote = Arc::new(Remote {
        history,
        sse,
        states: Mutex::new(Vec::new()),
        urls: Mutex::new(Vec::new()),
    });
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "remote", Arc::new(RemoteAdapter));
    let (service, events) = setup(db.clone(), adapters, remote.clone(), host(), true).await;
    events.register([&PROMPTED]);
    let mut bus = service.bus().subscribe();

    let info = service
        .create(
            CreateInput {
                id: Some("wrk_remote".into()),
                ..input("remote")
            },
            &AdapterContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(info.id, "wrk_remote");
    insert_session(&db, "ses_a", PROJECT, "/p", Some("wrk_remote"), None).await;

    let fence = BTreeMap::from([("ses_a".to_owned(), 2), ("ses_b".to_owned(), 0)]);
    service
        .wait_for_sync("wrk_remote", &fence, None, Some(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(events.latest_sequence("ses_a").await.unwrap(), 2);
    assert_eq!(events.latest_sequence("ses_b").await.unwrap(), 0);

    let urls = remote.urls.lock().unwrap().clone();
    assert_eq!(
        urls[0], "http://remote.test/base/global/event",
        "route drops query and joins path"
    );
    assert_eq!(urls[1], "http://remote.test/base/sync/history");
    assert_eq!(
        remote.states.lock().unwrap()[0],
        BTreeMap::new(),
        "no local sessions yet"
    );

    // Non-sync events are re-emitted on the bus tagged with the workspace; heartbeats are dropped.
    let mut seen = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(200), bus.recv()).await {
        seen.push((
            event.workspace.clone(),
            event.payload_type().map(str::to_owned),
            event.directory.clone(),
        ));
    }
    assert!(seen.contains(&(
        Some("wrk_remote".into()),
        Some("custom.thing".into()),
        Some("/remote".into())
    )));
    assert!(
        !seen
            .iter()
            .any(|(_, t, _)| t.as_deref() == Some("server.heartbeat"))
    );
    let owner = rows(
        &db,
        "SELECT owner_id FROM event_sequence WHERE aggregate_id = 'ses_a'",
    )
    .await;
    assert_eq!(
        text(&owner[0][0]),
        "wrk_remote",
        "replay claims ownership for the workspace"
    );

    service.shutdown().await;
}

#[tokio::test]
async fn wait_for_sync_times_out_and_aborts() {
    let (service, _) = setup(
        db().await,
        AdapterRegistry::default(),
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let fence = BTreeMap::from([("ses_x".to_owned(), 0)]);
    assert!(service.synced(&BTreeMap::new()).await.unwrap());
    let error = service
        .wait_for_sync("wrk_1", &fence, None, Some(Duration::from_millis(50)))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Timed out waiting for sync fence: {\"ses_x\":0}"
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let error = service
        .wait_for_sync("wrk_1", &fence, Some(cancel), None)
        .await
        .unwrap_err();
    assert!(matches!(error, LocationError::SyncAborted { .. }));
}

#[test]
fn sse_decoder_matches_parse_sse() {
    let mut decoder = SseDecoder::new();
    let mut out = decoder.push(b"id: 7\r\nretry: 250\r\ndata: not json\r\n\r\ndata: {\"a\":");
    out.extend(decoder.push(b"1}\n\n: comment\ndata: tail"));
    out.extend(decoder.finish());
    assert_eq!(
        out,
        vec![
            json!({ "type": "sse.message", "properties": { "data": "not json", "id": "7", "retry": 250 } }),
            json!({ "a": 1 }),
            json!({ "type": "sse.message", "properties": { "data": "tail", "id": "7", "retry": 250 } }),
        ]
    );
}
