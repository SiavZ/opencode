//! Purpose: publish and commit semantics ported from packages/core/test/event.test.ts:87-420:
//! projectors and commit hooks in one transaction, rollback, listener ordering and isolation,
//! seq increments, custom aggregate fields, definition versions.
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
//! Contract tests: event-store/tests/commit.rs

mod support;

use std::sync::{Arc, Mutex};

use opencode_event_api::{EventError, EventStore, LocationRef, PublishOptions, versioned_type};
use opencode_storage_api::Sql;
use serde_json::json;
use support::*;

fn log() -> Arc<Mutex<Vec<String>>> {
    Arc::new(Mutex::new(Vec::new()))
}

#[tokio::test]
async fn publishes_live_events_with_location_and_without_durable() {
    let h = harness().await;
    let location = LocationRef {
        directory: "project".into(),
        workspace_id: Some("wrk_test".into()),
    };
    let event = h
        .events
        .publish(
            &MESSAGE,
            json!({"text": "hello"}),
            PublishOptions {
                location: Some(location.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(event.event_type, "test.message");
    assert!(event.id.starts_with("evt_"));
    assert_eq!(event.location, Some(location));
    assert!(event.durable.is_none());

    let bare = h
        .events
        .publish(&MESSAGE, json!({"text": "x"}), Default::default())
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&bare)
            .unwrap()
            .contains("\"location\"")
    );
}

#[tokio::test]
async fn publishes_definition_version() {
    let h = harness().await;
    let event = h
        .events
        .publish(
            &VERSIONED,
            json!({"id": "one", "text": "hello"}),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(event.event_type, "test.versioned");
    assert_eq!(event.durable.unwrap().version, 2);
}

#[tokio::test]
async fn runs_projectors_inline_with_committed_payload() {
    let h = harness().await;
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    h.events.project(
        &SYNC,
        Arc::new(move |_: &dyn Sql, event| {
            sink.lock().unwrap().push(event.clone());
            Ok(())
        }),
    );
    let event = h
        .events
        .publish(
            &SYNC,
            json!({"id": "one", "text": "hello"}),
            Default::default(),
        )
        .await
        .unwrap();
    h.events
        .publish(
            &SYNC,
            json!({"id": "one", "text": "again"}),
            Default::default(),
        )
        .await
        .unwrap();
    let received = received.lock().unwrap();
    assert_eq!(received[0], event);
    assert_eq!(received[1].data, json!({"id": "one", "text": "again"}));
    assert_eq!(received[1].durable.as_ref().unwrap().seq, 1);
}

#[tokio::test]
async fn commits_local_operational_state_inside_the_event_transaction() {
    let h = harness().await;
    let order = log();
    let sink = order.clone();
    h.events.project(
        &SYNC,
        Arc::new(move |_: &dyn Sql, _| {
            sink.lock().unwrap().push("projector".into());
            Ok(())
        }),
    );
    let sink = order.clone();
    h.events
        .publish(
            &SYNC,
            json!({"id": id("agg"), "text": "hello"}),
            PublishOptions {
                commit: Some(Box::new(move |_: &dyn Sql, seq| {
                    sink.lock().unwrap().push(format!("commit:{seq}"));
                    Ok(())
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(*order.lock().unwrap(), ["projector", "commit:0"]);
}

#[derive(Debug, thiserror::Error)]
#[error("commit failed")]
struct HookFailed;

#[tokio::test]
async fn rolls_back_event_and_projector_when_local_commit_fails() {
    let h = harness().await;
    let aggregate = id("agg");
    h.exec("CREATE TABLE IF NOT EXISTS event_commit_probe (value text NOT NULL)")
        .await;
    h.events.project(
        &SYNC,
        Arc::new(|tx: &dyn Sql, _| {
            tx.execute(
                "INSERT INTO event_commit_probe (value) VALUES ('projected')",
                &[],
            )?;
            Ok(())
        }),
    );
    let error = h
        .events
        .publish(
            &SYNC,
            json!({"id": aggregate, "text": "hello"}),
            PublishOptions {
                commit: Some(Box::new(|_: &dyn Sql, _| Err(Box::new(HookFailed)))),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(message(&error).contains("commit failed"));
    assert!(error.projector_source::<HookFailed>().is_some());
    assert!(
        h.query("SELECT value FROM event_commit_probe", vec![])
            .await
            .is_empty()
    );
    assert!(h.seqs(&aggregate).await.is_empty());
    assert_eq!(h.sequence(&aggregate).await, None);
}

#[tokio::test]
async fn projector_failure_rolls_back_and_skips_wakes_and_listeners() {
    let h = harness().await;
    let aggregate = id("agg");
    let heard = log();
    let sink = heard.clone();
    let _unlisten = h.events.listen(Arc::new(move |event| {
        sink.lock().unwrap().push(event.id.clone());
        Ok(())
    }));
    h.events
        .project(&SYNC, Arc::new(|_: &dyn Sql, _| Err(Box::new(HookFailed))));
    let error = h
        .events
        .publish(
            &SYNC,
            json!({"id": aggregate, "text": "x"}),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, EventError::Projector { .. }));
    assert!(heard.lock().unwrap().is_empty());
    assert_eq!(h.events.latest_sequence(&aggregate).await.unwrap(), -1);
}

#[tokio::test]
async fn rejects_local_commit_hooks_on_live_only_events() {
    let h = harness().await;
    let error = h
        .events
        .publish(
            &MESSAGE,
            json!({"text": "hello"}),
            PublishOptions {
                commit: Some(Box::new(|_: &dyn Sql, _| Ok(()))),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(&error),
        "Local commit hooks require a durable event"
    );
}

#[tokio::test]
async fn runs_listeners_after_projectors_and_unsubscribes() {
    let h = harness().await;
    let order = log();
    let sink = order.clone();
    h.events.project(
        &SYNC,
        Arc::new(move |_: &dyn Sql, _| {
            sink.lock().unwrap().push("projector".into());
            Ok(())
        }),
    );
    let sink = order.clone();
    let unsubscribe = h.events.listen(Arc::new(move |_| {
        sink.lock().unwrap().push("listener".into());
        Ok(())
    }));
    h.events
        .publish(&SYNC, json!({"id": "one", "text": "a"}), Default::default())
        .await
        .unwrap();
    unsubscribe();
    h.events
        .publish(&SYNC, json!({"id": "one", "text": "b"}), Default::default())
        .await
        .unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        ["projector", "listener", "projector"]
    );
}

#[tokio::test]
async fn isolates_listener_failures_after_durable_commit() {
    let h = harness().await;
    let order = log();
    let _a = h
        .events
        .listen(Arc::new(|_| Err(Box::new(HookFailed) as _)));
    let _b = h.events.listen(Arc::new(|_| panic!("listener defect")));
    let sink = order.clone();
    let _c = h.events.listen(Arc::new(move |event| {
        sink.lock().unwrap().push(event.event_type.clone());
        Ok(())
    }));
    let event = h
        .events
        .publish(
            &SYNC,
            json!({"id": "one", "text": "hello"}),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(*order.lock().unwrap(), ["test.sync"]);
    assert!(event.durable.is_some());
}

#[tokio::test]
async fn keeps_live_only_listener_failures_fail_fast() {
    let h = harness().await;
    let _a = h
        .events
        .listen(Arc::new(|_| Err(Box::new(HookFailed) as _)));
    let error = h
        .events
        .publish(&MESSAGE, json!({"text": "hello"}), Default::default())
        .await
        .unwrap_err();
    assert!(error.projector_source::<HookFailed>().is_some());
}

#[tokio::test]
async fn notifies_listeners_only_after_the_event_is_committed() {
    let h = harness().await;
    let aggregate = id("agg");
    let committed = Arc::new(Mutex::new(Vec::new()));
    // A listener cannot await, so it records the id; the test then confirms the row was visible
    // to a separate read right after publish returned (commit happens before notify).
    let sink = committed.clone();
    let _l = h.events.listen(Arc::new(move |event| {
        sink.lock().unwrap().push(event.id.clone());
        Ok(())
    }));
    let event = h
        .events
        .publish(
            &SYNC,
            json!({"id": aggregate, "text": "c"}),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(*committed.lock().unwrap(), std::slice::from_ref(&event.id));
    let rows = h
        .query(
            "SELECT id, seq FROM event WHERE id = ?",
            vec![event.id.as_str().into()],
        )
        .await;
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn inserts_rows_and_increments_seq_per_aggregate() {
    let h = harness().await;
    let aggregate = id("agg");
    let other = id("agg");
    for text in ["first", "second"] {
        h.events
            .publish(
                &SYNC,
                json!({"id": aggregate, "text": text}),
                Default::default(),
            )
            .await
            .unwrap();
    }
    h.events
        .publish(&SYNC, json!({"id": other, "text": "x"}), Default::default())
        .await
        .unwrap();
    assert_eq!(h.seqs(&aggregate).await, [0, 1]);
    assert_eq!(h.seqs(&other).await, [0]);
    let rows = h
        .query(
            "SELECT type, aggregate_id FROM event WHERE aggregate_id = ?",
            vec![aggregate.as_str().into()],
        )
        .await;
    assert_eq!(
        rows[0].get::<String>("type").unwrap(),
        versioned_type("test.sync", 1)
    );
    assert_eq!(h.events.latest_sequence(&aggregate).await.unwrap(), 1);
    assert_eq!(h.events.latest_sequence("absent").await.unwrap(), -1);
}

#[tokio::test]
async fn uses_custom_aggregate_field_and_rejects_missing_field() {
    let h = harness().await;
    let aggregate = id("msg");
    h.events
        .publish(
            &SENT,
            json!({"messageID": aggregate, "text": "sent"}),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(h.seqs(&aggregate).await, [0]);
    let error = h
        .events
        .publish(&SENT, json!({"text": "no aggregate"}), Default::default())
        .await
        .unwrap_err();
    assert_eq!(message(&error), "Expected string aggregate field messageID");
}
