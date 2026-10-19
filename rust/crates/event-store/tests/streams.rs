//! Purpose: stream semantics ported from packages/core/test/event.test.ts:87-155,323-352,422-520:
//! typed/wildcard subscriptions, bounded overflow isolation, durable catch-up then live tail,
//! commit-vs-history handoff race, wake coalescing, live-only omission and wake cleanup on drop.
//! Scope: integration test against real SQLite (no fixtures). Timeouts only bound hangs.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/lib.rs
//! |-- event-store/tests/support/mod.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- futures::FutureExt
//! |-- futures::StreamExt
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::spawn
//! |-- tokio::sync::Notify
//! |-- tokio::task::yield_now
//! |-- tokio::test
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test uses its own temp database.
//! Contract tests: event-store/tests/streams.rs

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use opencode_event_api::{EventError, EventStore, EventStream, Payload};
use opencode_event_store::Options;
use serde_json::json;
use support::*;
use tokio::sync::Notify;

const LIMIT: Duration = Duration::from_secs(10);

async fn take(stream: EventStream, count: usize) -> Vec<Payload> {
    tokio::time::timeout(
        LIMIT,
        stream.take(count).map(|event| event.unwrap()).collect(),
    )
    .await
    .expect("stream produced events in time")
}

fn coordinates(events: &[Payload]) -> Vec<(i64, serde_json::Value)> {
    events
        .iter()
        .map(|event| (event.durable.as_ref().unwrap().seq, event.data.clone()))
        .collect()
}

#[tokio::test]
async fn publishes_to_typed_and_wildcard_subscriptions() {
    let h = harness().await;
    let typed = h.events.subscribe(&MESSAGE);
    let all = h.events.all();
    let other = h.events.subscribe(&SYNC);
    let event = h
        .events
        .publish(&MESSAGE, json!({"text": "hello"}), Default::default())
        .await
        .unwrap();
    assert_eq!(take(typed, 1).await, std::slice::from_ref(&event));
    assert_eq!(take(all, 1).await, [event]);
    assert!(other.into_future().now_or_never().is_none());
}

#[tokio::test]
async fn runs_projectors_before_streams() {
    let h = harness().await;
    let seen = Arc::new(AtomicBool::new(false));
    let flag = seen.clone();
    h.events.project(
        &SYNC,
        Arc::new(move |_: &dyn opencode_storage_api::Sql, _| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    let all = h.events.all();
    h.events
        .publish(&SYNC, json!({"id": "one", "text": "x"}), Default::default())
        .await
        .unwrap();
    take(all, 1).await;
    assert!(seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn ends_only_the_overflowing_bounded_subscriber() {
    let h = harness().await;
    let mut slow = h.events.all_bounded(1);
    let fast = h.events.all_bounded(8);
    let first = h
        .events
        .publish(&MESSAGE, json!({"text": "one"}), Default::default())
        .await
        .unwrap();
    // The slow consumer takes one event and then stalls (its queue holds capacity 1).
    assert_eq!(slow.next().await.unwrap().unwrap(), first);
    h.events
        .publish(&MESSAGE, json!({"text": "two"}), Default::default())
        .await
        .unwrap();
    h.events
        .publish(&MESSAGE, json!({"text": "overflow"}), Default::default())
        .await
        .unwrap();
    let last = h
        .events
        .publish(
            &MESSAGE,
            json!({"text": "still delivered"}),
            Default::default(),
        )
        .await
        .unwrap();

    let texts: Vec<_> = take(fast, 4)
        .await
        .into_iter()
        .map(|event| event.data["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["one", "two", "overflow", "still delivered"]);
    // The slow stream drains what it buffered, then ends with SubscriberOverflow.
    assert_eq!(slow.next().await.unwrap().unwrap().data["text"], "two");
    assert!(matches!(
        slow.next().await,
        Some(Err(EventError::SubscriberOverflow { capacity: 1 }))
    ));
    assert!(slow.next().await.is_none());
    assert_ne!(last.id, first.id);
}

#[tokio::test]
async fn replays_after_a_sequence_and_tails_new_events() {
    let h = harness().await;
    let session = id("ses");
    for text in ["zero", "one"] {
        h.events
            .publish(&REMOVED, removed(&session, text), Default::default())
            .await
            .unwrap();
    }
    let stream = h.events.durable(&session, Some(0));
    let tail = tokio::spawn(take(stream, 2));
    tokio::task::yield_now().await;
    h.events
        .publish(&REMOVED, removed(&session, "two"), Default::default())
        .await
        .unwrap();
    assert_eq!(
        coordinates(&tail.await.unwrap()),
        [(1, removed(&session, "one")), (2, removed(&session, "two"))]
    );
}

#[tokio::test]
async fn catches_events_published_during_replay_handoff() {
    let h = harness().await;
    let session = id("ses");
    h.events
        .publish(&REMOVED, removed(&session, "zero"), Default::default())
        .await
        .unwrap();
    let stream = h.events.durable(&session, None);
    h.events
        .publish(&REMOVED, removed(&session, "one"), Default::default())
        .await
        .unwrap();
    assert_eq!(
        coordinates(&take(stream, 2).await),
        [
            (0, removed(&session, "zero")),
            (1, removed(&session, "one"))
        ]
    );
}

#[tokio::test]
async fn retains_a_wake_committed_while_historical_replay_is_paused() {
    let read_started = Arc::new(Notify::new());
    let continue_read = Arc::new(Notify::new());
    let pause = Arc::new(AtomicBool::new(true));
    let hook = {
        let (read_started, continue_read, pause) =
            (read_started.clone(), continue_read.clone(), pause.clone());
        Arc::new(move |_: String| {
            let (read_started, continue_read, pause) =
                (read_started.clone(), continue_read.clone(), pause.clone());
            async move {
                if pause.load(Ordering::SeqCst) {
                    read_started.notify_one();
                    continue_read.notified().await;
                }
            }
            .boxed()
        })
    };
    let h = harness_with(Options {
        before_aggregate_read: Some(hook),
    })
    .await;
    let session = id("ses");
    let tail = tokio::spawn(take(h.events.durable(&session, None), 1));
    read_started.notified().await;
    pause.store(false, Ordering::SeqCst);
    h.events
        .publish(
            &REMOVED,
            removed(&session, "during handoff"),
            Default::default(),
        )
        .await
        .unwrap();
    continue_read.notify_one();
    assert_eq!(
        coordinates(&tail.await.unwrap()),
        [(0, removed(&session, "during handoff"))]
    );
}

#[tokio::test]
async fn coalesces_wakes_while_draining_every_committed_event() {
    let h = harness().await;
    let session = id("ses");
    let count = 64;
    let tail = tokio::spawn(take(h.events.durable(&session, None), count));
    tokio::task::yield_now().await;
    for index in 0..count {
        h.events
            .publish(
                &REMOVED,
                removed(&session, &index.to_string()),
                Default::default(),
            )
            .await
            .unwrap();
    }
    let expected: Vec<_> = (0..count)
        .map(|index| (index as i64, removed(&session, &index.to_string())))
        .collect();
    assert_eq!(coordinates(&tail.await.unwrap()), expected);
}

#[tokio::test]
async fn omits_live_only_events_from_durable_streams() {
    let h = harness().await;
    let session = id("ses");
    let tail = tokio::spawn(take(h.events.durable(&session, None), 1));
    tokio::task::yield_now().await;
    h.events
        .publish(&MESSAGE, json!({"text": "live only"}), Default::default())
        .await
        .unwrap();
    h.events
        .publish(&REMOVED, removed(&session, "durable"), Default::default())
        .await
        .unwrap();
    let events = tail.await.unwrap();
    assert_eq!(events[0].event_type, "message.removed");
}

#[tokio::test]
async fn concurrent_publishers_and_durable_reader_see_every_seq_once() {
    let h = harness().await;
    let session = id("ses");
    let count = 40;
    let tail = tokio::spawn(take(h.events.durable(&session, None), count));
    let writers: Vec<_> = (0..4)
        .map(|writer| {
            let events = h.events.clone();
            let session = session.clone();
            tokio::spawn(async move {
                for index in 0..count / 4 {
                    events
                        .publish(
                            &REMOVED,
                            removed(&session, &format!("{writer}-{index}")),
                            Default::default(),
                        )
                        .await
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.await.unwrap();
    }
    let seqs: Vec<_> = tail
        .await
        .unwrap()
        .iter()
        .map(|event| event.durable.as_ref().unwrap().seq)
        .collect();
    assert_eq!(seqs, (0..count as i64).collect::<Vec<_>>());
}

#[tokio::test]
async fn read_aggregate_pages_with_has_more_and_type_filter() {
    let h = harness().await;
    let session = id("ses");
    for text in ["a", "b", "c"] {
        h.events
            .publish(&REMOVED, removed(&session, text), Default::default())
            .await
            .unwrap();
    }
    let types = ["message.removed.1".to_owned()];
    let page = h
        .events
        .read_aggregate(&session, None, 2, &types)
        .await
        .unwrap();
    assert!(page.has_more);
    assert_eq!(coordinates(&page.events).len(), 2);
    assert_eq!(page.events[0].event_type, "message.removed");
    let rest = h
        .events
        .read_aggregate(&session, Some(1), 2, &types)
        .await
        .unwrap();
    assert!(!rest.has_more);
    assert_eq!(rest.events[0].durable.as_ref().unwrap().seq, 2);
    let none = h
        .events
        .read_aggregate(&session, None, 10, &["other.1".to_owned()])
        .await
        .unwrap();
    assert!(none.events.is_empty() && !none.has_more);
}

#[tokio::test]
async fn dropped_streams_stop_receiving_and_do_not_block_publish() {
    let h = harness().await;
    let session = id("ses");
    drop(h.events.durable(&session, None));
    drop(h.events.all());
    drop(h.events.subscribe(&REMOVED));
    drop(h.events.all_bounded(1));
    for text in ["a", "b", "c"] {
        h.events
            .publish(&REMOVED, removed(&session, text), Default::default())
            .await
            .unwrap();
    }
    assert_eq!(h.seqs(&session).await, [0, 1, 2]);
}
