//! Purpose: fixture socket tests for SSE streams: current `/api/event` (connected first,
//! heartbeat, overflow), durable `/api/session/:id/event` replay with `after` and reconnect, legacy
//! `/event` (directory filter, disposal) and `/global/event`, headers and split-frame decoding.
//! Scope: live local listener with an in-memory `EventSource`/`GlobalBus` (fixture, not adapter).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- protocol/src/sse.rs
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/services/mod.rs
//! |-- server/src/services/streams.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- app::router
//! |-- axum::body::Body::empty
//! |-- axum::http::Request::get
//! |-- futures_util::stream::BoxStream
//! |-- futures_util::stream::StreamExt
//! |-- futures_util::stream::poll_fn
//! |-- futures_util::stream::unfold
//! |-- reqwest::Response
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- str::to_string
//! |-- tokio::pin
//! |-- tokio::spawn
//! |-- tokio::sync::Notify
//! |-- tokio::sync::mpsc::UnboundedReceiver
//! |-- tokio::sync::mpsc::UnboundedSender
//! |-- tokio::sync::mpsc::unbounded_channel
//! |-- tokio::test
//! |-- tokio::time::Instant::now
//! |-- tokio::time::sleep
//! |-- tokio::time::timeout
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-based; real durable wake/replay semantics are tested in event-store.
//! Contract tests: this file.

mod support;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::{BoxStream, StreamExt};
use opencode_event_api::{Durable, EventError, EventStream, LocationRef, Payload};
use opencode_protocol::sse::{Decoder, Frame};
use opencode_server::app::{self, ServerConfig};
use opencode_server::services::Services;
use opencode_server::services::streams::{EventSource, GlobalBus, GlobalEvent, StreamServices};
use serde_json::{Value, json};
use tokio::sync::Notify;

/// One in-memory subscriber queue with an optional bound.
struct Sub {
    queue: Mutex<VecDeque<Payload>>,
    overflowed: Mutex<bool>,
    capacity: Option<usize>,
    notify: Notify,
}

#[derive(Default)]
struct Inner {
    subs: Mutex<Vec<Arc<Sub>>>,
    durable: Mutex<Vec<Payload>>,
    wake: Notify,
}

/// In-memory event source fixture (cheap clone, shared state).
#[derive(Clone, Default)]
struct FakeEvents(Arc<Inner>);

impl FakeEvents {
    fn publish(&self, payload: Payload) {
        if payload.durable.is_some() {
            self.0.durable.lock().unwrap().push(payload.clone());
            self.0.wake.notify_waiters();
        }
        for sub in self.0.subs.lock().unwrap().iter() {
            let mut queue = sub.queue.lock().unwrap();
            if sub.capacity.is_some_and(|c| queue.len() >= c) {
                *sub.overflowed.lock().unwrap() = true;
            } else {
                queue.push_back(payload.clone());
            }
            sub.notify.notify_one();
        }
    }

    fn subscribe(&self, capacity: Option<usize>) -> EventStream {
        let sub = Arc::new(Sub {
            queue: Mutex::default(),
            overflowed: Mutex::new(false),
            capacity,
            notify: Notify::new(),
        });
        self.0.subs.lock().unwrap().push(sub.clone());
        futures_util::stream::unfold(sub, |sub| async move {
            loop {
                let popped = sub.queue.lock().unwrap().pop_front();
                if let Some(item) = popped {
                    return Some((Ok(item), sub));
                }
                if *sub.overflowed.lock().unwrap() {
                    let capacity = sub.capacity.unwrap_or(0);
                    return Some((Err(EventError::SubscriberOverflow { capacity }), sub));
                }
                sub.notify.notified().await;
            }
        })
        .boxed()
    }
}

impl EventSource for FakeEvents {
    fn all_bounded(&self, capacity: usize) -> EventStream {
        self.subscribe(Some(capacity))
    }
    fn all(&self) -> EventStream {
        self.subscribe(None)
    }
    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream {
        let state = (
            after.unwrap_or(-1),
            aggregate_id.to_string(),
            self.0.clone(),
        );
        futures_util::stream::unfold(state, |(mut seq, aggregate, inner)| async move {
            loop {
                // Register the wake before reading, then reread (event.ts durable).
                let wake = inner.clone();
                let notified = wake.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let next = inner
                    .durable
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|p| {
                        p.durable
                            .as_ref()
                            .is_some_and(|d| d.aggregate_id == aggregate && d.seq > seq)
                    })
                    .cloned();
                if let Some(payload) = next {
                    seq = payload.durable.as_ref().map(|d| d.seq).unwrap_or(seq);
                    return Some((Ok(payload), (seq, aggregate, inner)));
                }
                notified.await;
            }
        })
        .boxed()
    }
}

#[derive(Default)]
struct FakeBus {
    senders: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<GlobalEvent>>>,
}

impl FakeBus {
    fn emit(&self, event: Value) {
        self.senders
            .lock()
            .unwrap()
            .retain(|s| s.send(event.clone()).is_ok());
    }
}

impl GlobalBus for FakeBus {
    fn subscribe(&self) -> BoxStream<'static, GlobalEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.senders.lock().unwrap().push(tx);
        tokio_stream_from(rx)
    }
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<GlobalEvent>,
) -> BoxStream<'static, GlobalEvent> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)).boxed()
}

fn payload(id: &str, kind: &str, directory: Option<&str>, durable: Option<(&str, i64)>) -> Payload {
    Payload {
        id: id.into(),
        metadata: None,
        event_type: kind.into(),
        durable: durable.map(|(aggregate, seq)| Durable {
            aggregate_id: aggregate.into(),
            seq,
            version: 1,
        }),
        location: directory.map(|d| LocationRef {
            directory: d.into(),
            workspace_id: None,
        }),
        data: json!({ "n": id }),
    }
}

async fn server(
    events: &FakeEvents,
    bus: Option<Arc<FakeBus>>,
) -> opencode_server::lifecycle::Listener {
    let mut services: Services = support::services(Arc::new(support::FakeSessions::default()));
    services.streams = StreamServices {
        events: Some(Arc::new(events.clone())),
        global: bus.map(|b| b as Arc<dyn GlobalBus>),
        ..StreamServices::default()
    };
    let config = ServerConfig {
        full_host: true,
        ..ServerConfig::default()
    };
    support::start(app::router(services, config)).await
}

/// Reads frames (and comments as `:` frames) until `count` frames arrived or the body ended.
async fn read_frames(response: reqwest::Response, count: usize) -> (Vec<Frame>, Vec<String>, bool) {
    let mut decoder = Decoder::default();
    let mut frames = Vec::new();
    let mut comments = Vec::new();
    let mut stream = response.bytes_stream();
    let mut errored = false;
    while frames.len() + comments.len() < count {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                comments.extend(
                    text.lines()
                        .filter(|l| l.starts_with(':'))
                        .map(str::to_string),
                );
                frames.extend(decoder.push(&text));
            }
            Ok(Some(Err(_))) => {
                errored = true;
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }
    (frames, comments, errored)
}

#[tokio::test]
async fn current_stream_headers_connected_then_live() {
    let events = &FakeEvents::default();
    let server = server(events, None).await;
    let response = support::client()
        .get(format!("{}/api/event", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["cache-control"], "no-cache, no-transform");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    events.publish(payload("evt_1", "session.updated", Some("/w"), None));
    let (frames, comments, _) = read_frames(response, 3).await;
    let connected: Value = serde_json::from_str(&frames[0].data).unwrap();
    assert_eq!(connected["type"], "server.connected");
    assert_eq!(connected["data"], json!({}));
    // The first heartbeat tick is immediate (Effect Stream.tick).
    assert_eq!(comments, vec![": heartbeat".to_string()]);
    let live: Value = serde_json::from_str(&frames[1].data).unwrap();
    assert_eq!(
        live,
        json!({"id":"evt_1","type":"session.updated","location":{"directory":"/w"},"data":{"n":"evt_1"}})
    );
    server.stop(true).await;
}

#[tokio::test]
async fn slow_consumer_overflow_ends_the_stream() {
    let events = &FakeEvents::default();
    let server = server(events, None).await;
    let response = support::client()
        .get(format!("{}/api/event", server.url()))
        .send()
        .await
        .unwrap();
    for i in 0..400 {
        events.publish(payload(&format!("evt_{i}"), "x", None, None));
    }
    let (frames, _, errored) = read_frames(response, 1000).await;
    assert!(errored, "overflow must abort the body");
    assert!(
        frames.len() <= 257,
        "at most connected + capacity frames: {}",
        frames.len()
    );
    server.stop(true).await;
}

#[tokio::test]
async fn session_events_replay_after_cursor_then_live_and_reconnect() {
    let events = &FakeEvents::default();
    let server = server(events, None).await;
    for seq in 0..3 {
        events.publish(payload(
            &format!("evt_{seq}"),
            "session.next.prompted",
            None,
            Some(("ses_known", seq)),
        ));
    }
    let url = format!("{}/api/session/ses_known/event?after=0", server.url());
    let response = support::client().get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let reader = tokio::spawn(read_frames(response, 3));
    tokio::time::sleep(Duration::from_millis(50)).await;
    events.publish(payload(
        "evt_3",
        "session.next.prompted",
        None,
        Some(("ses_known", 3)),
    ));
    let (frames, _, _) = reader.await.unwrap();
    let seqs: Vec<i64> = frames
        .iter()
        .map(|f| {
            serde_json::from_str::<Value>(&f.data).unwrap()["durable"]["seq"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);

    // Reconnect with the last seen seq: nothing is replayed twice.
    let response = support::client()
        .get(format!(
            "{}/api/session/ses_known/event?after=3",
            server.url()
        ))
        .send()
        .await
        .unwrap();
    let reader = tokio::spawn(read_frames(response, 1));
    tokio::time::sleep(Duration::from_millis(50)).await;
    events.publish(payload(
        "evt_4",
        "session.next.prompted",
        None,
        Some(("ses_known", 4)),
    ));
    let (frames, _, _) = reader.await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&frames[0].data).unwrap()["id"],
        "evt_4"
    );
    server.stop(true).await;
}

#[tokio::test]
async fn session_events_check_placement_and_query() {
    let events = &FakeEvents::default();
    let server = server(events, None).await;
    let missing = support::client()
        .get(format!("{}/api/session/ses_missing/event", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    let body: Value = missing.json().await.unwrap();
    assert_eq!(body["_tag"], "SessionNotFoundError");
    let bad = support::client()
        .get(format!(
            "{}/api/session/ses_known/event?after=-5",
            server.url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    server.stop(true).await;
}

#[tokio::test]
async fn legacy_event_filters_directory_and_ends_on_dispose() {
    let events = &FakeEvents::default();
    let bus = Arc::new(FakeBus::default());
    let server = server(events, Some(bus.clone())).await;
    let response = support::client()
        .get(format!("{}/event?directory=/work", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let reader = tokio::spawn(read_frames(response, 10));
    tokio::time::sleep(Duration::from_millis(50)).await;
    events.publish(payload("evt_other", "x", Some("/other"), None));
    events.publish(payload("evt_mine", "x", Some("/work"), None));
    tokio::time::sleep(Duration::from_millis(50)).await;
    bus.emit(json!({ "directory": "/work", "payload": { "id": "evt_d", "type": "server.instance.disposed", "properties": { "directory": "/work" } } }));
    let (frames, _, _) = reader.await.unwrap();
    let values: Vec<Value> = frames
        .iter()
        .map(|f| serde_json::from_str(&f.data).unwrap())
        .collect();
    assert_eq!(values[0]["type"], "server.connected");
    assert_eq!(values[0]["properties"], json!({}));
    assert_eq!(
        values[1],
        json!({"id":"evt_mine","type":"x","properties":{"n":"evt_mine"}})
    );
    assert_eq!(
        values[2],
        json!({"id":"evt_d","type":"server.instance.disposed","properties":{"directory":"/work"}})
    );
    assert_eq!(values.len(), 3, "stream ends after disposal");
    server.stop(true).await;
}

#[tokio::test]
async fn global_event_forwards_bus_payloads() {
    let events = &FakeEvents::default();
    let bus = Arc::new(FakeBus::default());
    let server = server(events, Some(bus.clone())).await;
    let response = support::client()
        .get(format!("{}/global/event", server.url()))
        .send()
        .await
        .unwrap();
    let reader = tokio::spawn(read_frames(response, 2));
    tokio::time::sleep(Duration::from_millis(50)).await;
    bus.emit(json!({ "directory": "global", "payload": { "type": "installation.updated", "properties": { "version": "1" } } }));
    let (frames, _, _) = reader.await.unwrap();
    let first: Value = serde_json::from_str(&frames[0].data).unwrap();
    assert_eq!(first["payload"]["type"], "server.connected");
    assert_eq!(
        frames[1].data,
        r#"{"directory":"global","payload":{"type":"installation.updated","properties":{"version":"1"}}}"#
    );
    server.stop(true).await;
}

#[test]
fn split_frames_decode_like_whole_frames() {
    let encoded = format!(
        "{}{}{}",
        Frame::message(r#"{"a":1}"#).encode(),
        ": heartbeat\n\n",
        Frame::message("line1\nline2").encode()
    );
    for split in 1..encoded.len() {
        let mut decoder = Decoder::default();
        let mut frames = decoder.push(&encoded[..split]);
        frames.extend(decoder.push(&encoded[split..]));
        assert_eq!(frames.len(), 2, "split at {split}");
        assert_eq!(frames[1].data, "line1\nline2");
    }
}

#[tokio::test(start_paused = true)]
async fn legacy_heartbeat_every_10s_with_first_tick_dropped() {
    let events = &FakeEvents::default();
    let bus = Arc::new(FakeBus::default());
    let services = Services {
        streams: StreamServices {
            events: Some(Arc::new(events.clone())),
            global: Some(bus.clone()),
            ..StreamServices::default()
        },
        ..Services::default()
    };
    let router = app::router(
        services,
        ServerConfig {
            full_host: true,
            ..ServerConfig::default()
        },
    );
    // Drive the router in-process so paused time controls the heartbeat deterministically.
    use tower::ServiceExt;
    let request = axum::http::Request::get("/global/event")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let mut body = response.into_body().into_data_stream();
    let start = tokio::time::Instant::now();
    let first = String::from_utf8(body.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(first.contains("server.connected"));
    let second = String::from_utf8(body.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(second.contains("server.heartbeat"));
    assert_eq!(start.elapsed(), Duration::from_secs(10));
    // The bus outlives the router here; dropping it ends the source and halts the stream.
    drop(bus);
}
