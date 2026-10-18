//! Purpose: context epoch and history selection against a real SQLite file: initialize, unchanged
//! reuse, atomic ContextUpdated + snapshot advancement, rollback on missing epoch, incompatible
//! snapshot replacement, post-compaction replacement, unavailable blocking, decode errors, and
//! history selection after reopening the database (restart).
//! Scope: integration tests. Fixture-backed storage/event store (tests/support), real SQLite.
//! Uses:
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/src/history.rs
//! |-- context/src/source/mod.rs
//! |-- context/tests/support/mod.rs
//! |-- event-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/core/src/session/{context-epoch,history}.ts and the epoch cases of
//! packages/core/test/session-runner.test.ts.
//! Contract tests: this file

mod support;

use std::sync::{Arc, Mutex};

use opencode_context::epoch::{self, Prepared};
use opencode_context::history;
use opencode_context::source::{ContextError, Key, Observation, Source, SystemContext, make};
use opencode_event_api::EventStore;
use opencode_storage_api::Value;
use serde_json::json;
use support::{TestDb, TestEvents, append, insert_session};

const SID: &str = "ses_epoch";

#[derive(Clone)]
struct Live(Arc<Mutex<Option<String>>>);

impl Live {
    fn new(v: &str) -> Self {
        Self(Arc::new(Mutex::new(Some(v.into()))))
    }
    fn set(&self, v: Option<&str>) {
        *self.0.lock().unwrap() = v.map(String::from);
    }
    fn context(&self) -> SystemContext {
        let cell = self.0.clone();
        make(
            Source::<String>::new(
                Key::parse("test/context").unwrap(),
                move || {
                    let v = cell.lock().unwrap().clone();
                    async move { Ok(v.map_or(Observation::Unavailable, Observation::Available)) }
                },
                |v| v.clone(),
                |_, c| c.clone(),
            )
            .with_removed(|_| "System context source removed: test/context".into()),
        )
    }
}

fn setup() -> (tempfile::TempDir, Arc<TestDb>, TestEvents) {
    let dir = tempfile::tempdir().unwrap();
    let db = TestDb::create(&dir.path().join("opencode.db"));
    insert_session(&db, SID);
    let events = TestEvents::new(db.clone());
    (dir, db, events)
}

async fn prepare(db: &TestDb, events: &TestEvents, live: &Live) -> Result<Prepared, ContextError> {
    let ctx = live.context();
    epoch::prepare(db, events, async move { Ok(ctx) }, SID).await
}

fn epoch_row(db: &TestDb) -> Option<epoch::StoredEpoch> {
    db.with(|sql| epoch::find(sql, SID)).unwrap()
}

fn context_updates(db: &TestDb) -> Vec<String> {
    db.with(|sql| {
        sql.query(
            "SELECT data FROM event WHERE type = 'session.next.context.updated.1' ORDER BY seq",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| {
            r.get_json::<serde_json::Value>("data").unwrap()["text"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
    })
}

#[tokio::test]
async fn initializes_once_with_latest_sequence_as_baseline() {
    let (_dir, db, events) = setup();
    append(
        &events,
        SID,
        Some(("msg_1", "user", json!({"text": "First"}))),
    )
    .await;
    let live = Live::new("Initial context");
    let ctx = live.context();
    let first = epoch::initialize(&*db, async move { Ok(ctx) }, SID)
        .await
        .unwrap();
    assert_eq!(
        first,
        Some(Prepared {
            baseline: "Initial context".into(),
            baseline_seq: 0
        })
    );
    let ctx = live.context();
    assert_eq!(
        epoch::initialize(&*db, async move { Ok(ctx) }, SID)
            .await
            .unwrap(),
        None
    );
    let row = epoch_row(&db).unwrap();
    assert_eq!(
        row.snapshot,
        r#"{"test/context":{"value":"Initial context","removed":"System context source removed: test/context"}}"#
    );
}

#[tokio::test]
async fn initialization_blocked_writes_nothing() {
    let (_dir, db, events) = setup();
    let live = Live::new("x");
    live.set(None);
    let err = prepare(&db, &events, &live).await.unwrap_err();
    assert!(matches!(err, ContextError::InitializationBlocked { .. }));
    assert!(epoch_row(&db).is_none());
    live.set(Some("Initial context"));
    assert_eq!(
        prepare(&db, &events, &live).await.unwrap().baseline,
        "Initial context"
    );
}

#[tokio::test]
async fn reuses_baseline_and_publishes_one_update_atomically() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    let first = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(
        prepare(&db, &events, &live).await.unwrap(),
        first,
        "unchanged keeps baseline"
    );
    assert!(context_updates(&db).is_empty());

    live.set(Some("Changed context"));
    let second = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(second, first, "baseline text and seq survive an update");
    assert_eq!(context_updates(&db), vec!["Changed context"]);
    let snapshot: serde_json::Value =
        serde_json::from_str(&epoch_row(&db).unwrap().snapshot).unwrap();
    assert_eq!(snapshot["test/context"]["value"], "Changed context");

    // Snapshot advanced with the event, so the next turn does not repeat the update.
    prepare(&db, &events, &live).await.unwrap();
    assert_eq!(context_updates(&db).len(), 1);
}

#[tokio::test]
async fn update_rolls_back_event_when_epoch_vanishes() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    prepare(&db, &events, &live).await.unwrap();
    live.set(Some("Changed"));
    // The registered fixture projector deletes the epoch before the commit hook, like a racing reset.
    events.project(
        &epoch::CONTEXT_UPDATED,
        Arc::new(|sql, _| {
            sql.execute("DELETE FROM session_context_epoch", &[])?;
            Ok(())
        }),
    );
    let err = prepare(&db, &events, &live).await.unwrap_err();
    assert!(matches!(err, ContextError::EpochNotFound { .. }), "{err}");
    assert!(
        context_updates(&db).is_empty(),
        "event rolled back with the failed snapshot advance"
    );
    assert!(epoch_row(&db).is_some(), "projector delete rolled back too");
}

#[tokio::test]
async fn incompatible_snapshot_replaces_generation_at_latest_seq() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    prepare(&db, &events, &live).await.unwrap();
    append(&events, SID, None).await;
    append(&events, SID, None).await;
    db.with(|sql| {
        sql.execute(
            "UPDATE session_context_epoch SET snapshot = ? WHERE session_id = ?",
            &[
                Value::from(r#"{"test/context":{"value":42}}"#),
                Value::from(SID),
            ],
        )
    })
    .unwrap();
    live.set(Some("Replaced"));
    let prepared = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(
        prepared,
        Prepared {
            baseline: "Replaced".into(),
            baseline_seq: 1
        }
    );
    assert!(context_updates(&db).is_empty());
}

#[tokio::test]
async fn invalid_snapshot_fails_with_decode_error() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    prepare(&db, &events, &live).await.unwrap();
    db.with(|sql| {
        sql.execute(
            "UPDATE session_context_epoch SET snapshot = ? WHERE session_id = ?",
            &[
                Value::from(r#"{"invalid":{"value":"bad"}}"#),
                Value::from(SID),
            ],
        )
    })
    .unwrap();
    let err = prepare(&db, &events, &live).await.unwrap_err();
    assert!(matches!(err, ContextError::SnapshotDecode { .. }), "{err}");
    assert!(
        err.to_string()
            .starts_with("Failed to decode context snapshot for session ses_epoch")
    );
}

#[tokio::test]
async fn compaction_after_baseline_rebuilds_at_compaction_seq() {
    let (_dir, db, events) = setup();
    append(
        &events,
        SID,
        Some(("msg_u1", "user", json!({"text": "one"}))),
    )
    .await;
    let live = Live::new("Initial context");
    let first = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(first.baseline_seq, 0);
    append(&events, SID, Some(("msg_a1", "assistant", json!({})))).await;
    let compaction = append(
        &events,
        SID,
        Some(("msg_c", "compaction", json!({"summary": "s"}))),
    )
    .await;
    append(&events, SID, Some(("msg_a2", "assistant", json!({})))).await;

    live.set(Some("Fresh context"));
    let prepared = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(
        prepared,
        Prepared {
            baseline: "Fresh context".into(),
            baseline_seq: compaction
        }
    );
    assert!(
        context_updates(&db).is_empty(),
        "replacement does not publish an update"
    );

    let rows = db
        .with(|sql| history::entries_for_runner(sql, SID, prepared.baseline_seq))
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.message_type.as_str())
            .collect::<Vec<_>>(),
        vec!["compaction", "assistant"]
    );
}

#[tokio::test]
async fn compaction_replacement_blocks_while_admitted_source_unavailable() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    let first = prepare(&db, &events, &live).await.unwrap();
    append(&events, SID, Some(("msg_c", "compaction", json!({})))).await;
    live.set(None);
    assert_eq!(prepare(&db, &events, &live).await.unwrap(), first);
}

#[tokio::test]
async fn history_selection_survives_restart() {
    let (dir, db, events) = setup();
    append(
        &events,
        SID,
        Some(("msg_u1", "user", json!({"text": "old"}))),
    )
    .await;
    append(
        &events,
        SID,
        Some(("msg_s0", "system", json!({"text": "in baseline"}))),
    )
    .await;
    let live = Live::new("Initial context");
    let prepared = prepare(&db, &events, &live).await.unwrap();
    assert_eq!(prepared.baseline_seq, 1);
    append(
        &events,
        SID,
        Some(("msg_c", "compaction", json!({"summary": "s"}))),
    )
    .await;
    append(
        &events,
        SID,
        Some(("msg_u2", "user", json!({"text": "new"}))),
    )
    .await;
    // A post-baseline system message older than nothing else: still selected.
    append(
        &events,
        SID,
        Some(("msg_s1", "system", json!({"text": "Changed"}))),
    )
    .await;
    drop(events);
    drop(db);

    let db = TestDb::open(&dir.path().join("opencode.db"));
    let visible = db.with(|sql| history::load(sql, SID)).unwrap();
    assert_eq!(
        visible.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["msg_c", "msg_u2", "msg_s1"]
    );
    assert_eq!(
        visible[1].message_json(),
        json!({"text": "new", "id": "msg_u2", "type": "user"})
    );

    // Without a compaction: all non-system rows plus post-baseline system rows.
    let rows = db
        .with(|sql| history::message_rows(sql, SID, None, Some(1)))
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["msg_u1", "msg_c", "msg_u2", "msg_s1"]
    );
    // No epoch and no compaction: everything.
    let all = db
        .with(|sql| history::message_rows(sql, SID, None, None))
        .unwrap();
    assert_eq!(all.len(), 5);
}

#[tokio::test]
async fn post_baseline_system_messages_before_compaction_are_kept() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    prepare(&db, &events, &live).await.unwrap();
    append(&events, SID, Some(("msg_u1", "user", json!({})))).await;
    append(&events, SID, Some(("msg_s1", "system", json!({})))).await;
    let c = append(&events, SID, Some(("msg_c", "compaction", json!({})))).await;
    let rows = db
        .with(|sql| history::message_rows(sql, SID, Some(c), Some(-1)))
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["msg_s1", "msg_c"]
    );
}

#[tokio::test]
async fn reset_deletes_epoch() {
    let (_dir, db, events) = setup();
    let live = Live::new("Initial context");
    prepare(&db, &events, &live).await.unwrap();
    epoch::reset(&*db, SID).await.unwrap();
    assert!(epoch_row(&db).is_none());
}
