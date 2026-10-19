//! Purpose: replay, replayAll and remove semantics ported from packages/core/test/event.test.ts:
//! 540-760, 961-1032, 1100-1124: exact duplicate no-ops, divergence, sequence gaps, duplicate IDs
//! and partial replayAll failure. Owner fencing lives in owners.rs.
//! Scope: integration test against real SQLite (no fixtures).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/tests/support/mod.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test uses its own temp database.
//! Contract tests: event-store/tests/replay.rs

mod support;

use std::sync::{Arc, Mutex};

use opencode_event_api::{EventStore, Payload, ReplayOptions};
use opencode_storage_api::Sql;
use serde_json::json;
use support::*;

fn recorder(h: &Harness) -> Arc<Mutex<Vec<Payload>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    h.events.project(
        &REMOVED,
        Arc::new(move |_: &dyn Sql, event| {
            sink.lock().unwrap().push(event.clone());
            Ok(())
        }),
    );
    received
}

fn listener(h: &Harness) -> Arc<Mutex<Vec<Payload>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    // The unsubscribe handle is intentionally leaked: dropping it does not unsubscribe.
    let _ = h.events.listen(Arc::new(move |event| {
        sink.lock().unwrap().push(event.clone());
        Ok(())
    }));
    received
}

#[tokio::test]
async fn replays_through_projectors_and_inserts_rows() {
    let h = harness().await;
    let received = recorder(&h);
    let session = id("ses");
    h.events
        .replay(
            serialized(&id("evt"), &session, 0, removed(&session, "hello")),
            Default::default(),
        )
        .await
        .unwrap();
    let first = received.lock().unwrap()[0].clone();
    assert_eq!(first.event_type, "message.removed");
    assert_eq!(first.data, removed(&session, "hello"));
    assert_eq!(first.durable.as_ref().unwrap().seq, 0);
    assert_eq!(h.seqs(&session).await, [0]);
}

#[tokio::test]
async fn rejects_envelope_aggregate_that_differs_from_payload() {
    let h = harness().await;
    let envelope = id("ses");
    let payload = id("ses");
    h.events
        .publish(&REMOVED, removed(&payload, "seed"), Default::default())
        .await
        .unwrap();
    let received = recorder(&h);
    let error = h
        .events
        .replay(
            serialized(&id("evt"), &envelope, 1, removed(&payload, "replayed")),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Aggregate mismatch: expected {envelope}, got {payload}")
    );
    assert!(received.lock().unwrap().is_empty());
    assert_eq!(h.seqs(&payload).await, [0]);
    assert_eq!(h.sequence(&payload).await.unwrap().0, 0);
}

#[tokio::test]
async fn rejects_sequence_gaps_without_writing() {
    let h = harness().await;
    let session = id("ses");
    h.events
        .replay(
            serialized(&id("evt"), &session, 0, removed(&session, "first")),
            Default::default(),
        )
        .await
        .unwrap();
    let error = h
        .events
        .replay(
            serialized(&id("evt"), &session, 5, removed(&session, "bad")),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Sequence mismatch for aggregate {session}: expected 1, got 5")
    );
    assert_eq!(h.seqs(&session).await, [0]);
}

#[tokio::test]
async fn rejects_unknown_event_types() {
    let h = harness().await;
    let mut event = serialized(&id("evt"), "agg", 0, json!({}));
    event.event_type = "unknown.event.1".into();
    let error = h
        .events
        .replay(event, Default::default())
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        "Unknown durable event type unknown.event.1"
    );
}

#[tokio::test]
async fn replay_all_validates_contiguous_single_aggregate_batches() {
    let h = harness().await;
    let session = id("ses");
    let one = h
        .events
        .replay_all(
            vec![
                serialized(&id("evt"), &session, 0, removed(&session, "one")),
                serialized(&id("evt"), &session, 1, removed(&session, "two")),
            ],
            Default::default(),
        )
        .await
        .unwrap();
    let two = h
        .events
        .replay_all(
            vec![
                serialized(&id("evt"), &session, 2, removed(&session, "three")),
                serialized(&id("evt"), &session, 3, removed(&session, "four")),
            ],
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(one.as_deref(), Some(session.as_str()));
    assert_eq!(two.as_deref(), Some(session.as_str()));
    assert_eq!(h.seqs(&session).await, [0, 1, 2, 3]);
    assert_eq!(
        h.events
            .replay_all(vec![], Default::default())
            .await
            .unwrap(),
        None
    );

    let other = id("ses");
    let error = h
        .events
        .replay_all(
            vec![
                serialized(&id("evt"), &other, 0, removed(&other, "a")),
                serialized(&id("evt"), &session, 4, removed(&session, "b")),
            ],
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        "Replay events must belong to the same aggregate"
    );
    let error = h
        .events
        .replay_all(
            vec![
                serialized(&id("evt"), &other, 0, removed(&other, "a")),
                serialized(&id("evt"), &other, 2, removed(&other, "b")),
            ],
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        "Replay sequence mismatch at index 1: expected 1, got 2"
    );
    assert!(h.seqs(&other).await.is_empty());
}

#[tokio::test]
async fn replay_all_partial_failure_keeps_earlier_events_committed() {
    let h = harness().await;
    let session = id("ses");
    let reused = id("evt");
    h.events
        .replay(
            serialized(&reused, &session, 0, removed(&session, "zero")),
            Default::default(),
        )
        .await
        .unwrap();
    // Contiguous batch whose third event reuses an existing ID: events 1 and 2 commit first.
    let error = h
        .events
        .replay_all(
            vec![
                serialized(&id("evt"), &session, 1, removed(&session, "one")),
                serialized(&id("evt"), &session, 2, removed(&session, "two")),
                serialized(&reused, &session, 3, removed(&session, "three")),
            ],
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Event {reused} already exists at aggregate {session} sequence 0")
    );
    assert_eq!(h.seqs(&session).await, [0, 1, 2]);
    assert_eq!(h.events.latest_sequence(&session).await.unwrap(), 2);
}

#[tokio::test]
async fn publishes_accepted_replay_once_and_suppresses_exact_duplicates() {
    let h = harness().await;
    let received = listener(&h);
    let session = id("ses");
    let event = serialized(&id("evt"), &session, 0, removed(&session, "replayed"));
    let publish = ReplayOptions {
        publish: true,
        ..Default::default()
    };
    h.events
        .replay(event.clone(), publish.clone())
        .await
        .unwrap();
    h.events.replay(event.clone(), publish).await.unwrap();
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].id, event.id);
    let durable = received[0].durable.as_ref().unwrap();
    assert_eq!((durable.seq, durable.version), (0, 1));
    assert_eq!(received[0].data, event.data);
}

#[tokio::test]
async fn rejects_divergent_stale_replay_without_publishing() {
    let h = harness().await;
    let received = listener(&h);
    let session = id("ses");
    let event = serialized(&id("evt"), &session, 0, removed(&session, "original"));
    let publish = ReplayOptions {
        publish: true,
        ..Default::default()
    };
    h.events
        .replay(event.clone(), publish.clone())
        .await
        .unwrap();
    let mut divergent = event.clone();
    divergent.data = removed(&session, "divergent");
    let error = h
        .events
        .replay(divergent, publish.clone())
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Replay diverged at aggregate {session} sequence 0")
    );
    // Different ID at an occupied position also diverges.
    let mut other_id = event;
    other_id.id = id("evt");
    let error = h.events.replay(other_id, publish).await.unwrap_err();
    assert!(message(&error).starts_with("Replay diverged"));
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rejects_event_id_reused_at_another_position() {
    let h = harness().await;
    let session = id("ses");
    let reused = id("evt");
    h.events
        .replay(
            serialized(&reused, &session, 0, removed(&session, "first")),
            Default::default(),
        )
        .await
        .unwrap();
    let error = h
        .events
        .replay(
            serialized(&reused, &session, 1, removed(&session, "second")),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Event {reused} already exists at aggregate {session} sequence 0")
    );
}

#[tokio::test]
async fn remove_clears_the_aggregate_for_fresh_replay() {
    let h = harness().await;
    let session = id("ses");
    h.events
        .publish(&REMOVED, removed(&session, "seed"), Default::default())
        .await
        .unwrap();
    h.events.remove(&session).await.unwrap();
    assert!(h.seqs(&session).await.is_empty());
    assert_eq!(h.sequence(&session).await, None);
    let received = recorder(&h);
    h.events
        .replay(
            serialized(&id("evt"), &session, 0, removed(&session, "replayed")),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        received.lock().unwrap()[0].data,
        removed(&session, "replayed")
    );
}
