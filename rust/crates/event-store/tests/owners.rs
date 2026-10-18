//! Purpose: replay owner fencing and claims ported from packages/core/test/event.test.ts:776-960,
//! 1033-1098: strict vs non-strict owner mismatch, owner backfill on exact replay, claim of
//! unowned sequences, absent-row claims.
//! Scope: integration test against real SQLite (no fixtures).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/lib.rs
//! `-- event-store/tests/support/mod.rs
//! External:
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test uses its own temp database.
//! Contract tests: event-store/tests/owners.rs

mod support;

use std::sync::{Arc, Mutex};

use opencode_event_api::{EventStore, Payload, ReplayOptions};
use opencode_storage_api::Sql;
use serde_json::json;
use support::*;

fn owner(owner: &str) -> ReplayOptions {
    ReplayOptions {
        owner_id: Some(owner.into()),
        ..Default::default()
    }
}

fn strict(owner: &str) -> ReplayOptions {
    ReplayOptions {
        owner_id: Some(owner.into()),
        strict_owner: true,
        ..Default::default()
    }
}

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
async fn claim_fences_non_strict_replay_owners() {
    let h = harness().await;
    let session = id("ses");
    h.events
        .publish(&REMOVED, removed(&session, "seed"), Default::default())
        .await
        .unwrap();
    h.events.claim(&session, "owner-a").await.unwrap();
    let received = recorder(&h);
    h.events
        .replay(
            serialized(&id("evt"), &session, 1, removed(&session, "ignored")),
            owner("owner-b"),
        )
        .await
        .unwrap();
    assert!(received.lock().unwrap().is_empty());
    assert_eq!(h.seqs(&session).await, [0]);
}

#[tokio::test]
async fn strict_owner_fences_exact_replay() {
    let h = harness().await;
    let session = id("ses");
    let event = serialized(&id("evt"), &session, 0, removed(&session, "owned"));
    h.events
        .replay(event.clone(), owner("owner-a"))
        .await
        .unwrap();
    let error = h.events.replay(event, strict("owner-b")).await.unwrap_err();
    assert_eq!(
        message(&error),
        format!("Replay owner mismatch for aggregate {session}: expected owner-a, got owner-b")
    );
    let error = h
        .events
        .replay(
            serialized(&id("evt"), &session, 1, removed(&session, "x")),
            ReplayOptions {
                strict_owner: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        format!("Replay owner mismatch for aggregate {session}: expected owner-a, got none")
    );
}

#[tokio::test]
async fn exact_replay_claims_an_unowned_aggregate() {
    let h = harness().await;
    let session = id("ses");
    let published = h
        .events
        .publish(&REMOVED, removed(&session, "owned"), Default::default())
        .await
        .unwrap();
    let replayed = serialized(
        &published.id,
        &session,
        published.durable.as_ref().unwrap().seq,
        published.data.clone(),
    );
    h.events
        .replay(replayed.clone(), strict("owner-a"))
        .await
        .unwrap();
    assert_eq!(
        h.sequence(&session).await,
        Some((0, Some("owner-a".into())))
    );
    let error = h
        .events
        .replay(
            serialized(&id("evt"), &session, 1, removed(&session, "conflict")),
            strict("owner-b"),
        )
        .await
        .unwrap_err();
    assert!(message(&error).contains("Replay owner mismatch"));
}

#[tokio::test]
async fn replay_with_owner_claims_new_and_existing_unowned_sequences() {
    let h = harness().await;
    let fresh = id("ses");
    h.events
        .replay(
            serialized(&id("evt"), &fresh, 0, removed(&fresh, "owned")),
            owner("owner-1"),
        )
        .await
        .unwrap();
    assert_eq!(h.sequence(&fresh).await, Some((0, Some("owner-1".into()))));

    let local = id("ses");
    h.events
        .publish(&REMOVED, removed(&local, "local"), Default::default())
        .await
        .unwrap();
    h.events
        .replay(
            serialized(&id("evt"), &local, 1, removed(&local, "claimed")),
            owner("owner-1"),
        )
        .await
        .unwrap();
    h.events
        .replay(
            serialized(&id("evt"), &local, 2, removed(&local, "fenced")),
            owner("owner-2"),
        )
        .await
        .unwrap();
    assert_eq!(h.seqs(&local).await, [0, 1]);
    assert_eq!(h.sequence(&local).await, Some((1, Some("owner-1".into()))));
}

#[tokio::test]
async fn strict_replay_rejects_owner_conflicts_instead_of_skipping() {
    let h = harness().await;
    let session = id("ses");
    h.events
        .replay(
            serialized(&id("evt"), &session, 0, removed(&session, "claimed")),
            owner("owner-1"),
        )
        .await
        .unwrap();
    let error = h
        .events
        .replay(
            serialized(&id("evt"), &session, 1, removed(&session, "conflict")),
            strict("owner-2"),
        )
        .await
        .unwrap_err();
    assert!(message(&error).contains("Replay owner mismatch"));
}

#[tokio::test]
async fn replay_from_another_owner_leaves_claimed_sequence_unchanged() {
    let h = harness().await;
    let received = listener(&h);
    let session = id("ses");
    h.events
        .replay(
            serialized(&id("evt"), &session, 0, removed(&session, "first")),
            owner("owner-1"),
        )
        .await
        .unwrap();
    h.events
        .replay(
            serialized(&id("evt"), &session, 1, removed(&session, "ignored")),
            ReplayOptions {
                owner_id: Some("owner-2".into()),
                publish: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(h.seqs(&session).await, [0]);
    assert_eq!(
        h.sequence(&session).await,
        Some((0, Some("owner-1".into())))
    );
    assert!(received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn claim_updates_owner_and_leaves_absent_rows_absent() {
    let h = harness().await;
    let aggregate = id("agg");
    h.events
        .publish(
            &SYNC,
            json!({"id": aggregate, "text": "claimed"}),
            Default::default(),
        )
        .await
        .unwrap();
    h.events.claim(&aggregate, "owner-1").await.unwrap();
    h.events.claim(&aggregate, "owner-2").await.unwrap();
    assert_eq!(
        h.sequence(&aggregate).await,
        Some((0, Some("owner-2".into())))
    );
    h.events.claim("absent", "owner-1").await.unwrap();
    assert_eq!(h.sequence("absent").await, None);
}
