//! Purpose: exercise history boundaries, restarted context updates and publication rollback on real SQLite.
//! Scope: integration tests with fixture-labeled Database/EventStore adapters and system projector.
//! Uses:
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/src/history.rs
//! |-- context/src/source/mod.rs
//! |-- context/tests/support/mod.rs
//! |-- event-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- epoch::CONTEXT_UPDATED
//! |-- epoch::find
//! |-- epoch::prepare
//! |-- futures::FutureExt
//! |-- futures::future::pending
//! |-- history::MessageRow
//! |-- history::load
//! |-- history::message_rows
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: only selected raw rows survive restart, and failed durable writes roll back history and snapshot together.
//! Contract tests: this file

mod support;

use std::sync::Arc;

use futures::FutureExt;
use opencode_context::epoch::{self, Prepared};
use opencode_context::history;
use opencode_context::source::{ContextError, Key, Observation, Source, SystemContext, make};
use opencode_event_api::EventStore;
use opencode_storage_api::Value;
use serde_json::json;
use support::{TestDb, TestEvents, append, insert_session};

const SID: &str = "ses_history";

fn context(value: Option<&str>) -> SystemContext {
    let value = value.map(String::from);
    make(
        Source::new(
            Key::parse("test/context").unwrap(),
            move || {
                let value = value.clone();
                async move { Ok(value.map_or(Observation::Unavailable, Observation::Available)) }
            },
            Clone::clone,
            |_, current| current.clone(),
        )
        .with_removed(|_| "Context removed".into()),
    )
}

fn setup() -> (tempfile::TempDir, Arc<TestDb>, TestEvents) {
    let dir = tempfile::tempdir().unwrap();
    let db = TestDb::create(&dir.path().join("history.db"));
    insert_session(&db, SID);
    let events = TestEvents::new(db.clone());
    events.project_system_updates();
    (dir, db, events)
}

async fn prepare(db: &TestDb, events: &TestEvents, value: Option<&str>) -> Prepared {
    let ctx = context(value);
    epoch::prepare(db, events, async { Ok(ctx) }, SID)
        .await
        .unwrap()
}

fn ids(rows: &[history::MessageRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

#[tokio::test]
async fn history_selection_covers_all_baseline_and_compaction_boundaries() {
    let (_dir, db, events) = setup();
    append(
        &events,
        SID,
        Some(("old_user", "user", json!({"text": "old"}))),
    )
    .await;
    append(
        &events,
        SID,
        Some(("baseline_system", "system", json!({"text": "covered"}))),
    )
    .await;
    assert_eq!(prepare(&db, &events, Some("Initial")).await.baseline_seq, 1);
    append(
        &events,
        SID,
        Some(("before_compaction", "system", json!({"text": "effective"}))),
    )
    .await;
    append(
        &events,
        SID,
        Some(("old_compaction", "compaction", json!({"summary": "old"}))),
    )
    .await;
    append(
        &events,
        SID,
        Some((
            "middle_user",
            "user",
            json!({"id": "spoof", "type": "system", "text": "middle"}),
        )),
    )
    .await;
    append(
        &events,
        SID,
        Some(("latest_compaction", "compaction", json!({"summary": "new"}))),
    )
    .await;
    append(&events, SID, Some(("assistant", "assistant", json!({})))).await;
    append(
        &events,
        SID,
        Some(("after_compaction", "system", json!({"text": "after"}))),
    )
    .await;
    insert_session(&db, "ses_other");
    append(
        &events,
        "ses_other",
        Some(("other_compaction", "compaction", json!({}))),
    )
    .await;

    db.with(|sql| {
        assert_eq!(history::latest_compaction(sql, SID).unwrap(), Some(5));
        let loaded = history::load(sql, SID).unwrap();
        assert_eq!(
            ids(&loaded),
            [
                "before_compaction",
                "latest_compaction",
                "assistant",
                "after_compaction"
            ]
        );
        assert_eq!(
            loaded.iter().map(|row| row.seq).collect::<Vec<_>>(),
            [2, 5, 6, 7]
        );
        assert_eq!(
            ids(&history::entries_for_runner(sql, SID, 5).unwrap()),
            ["latest_compaction", "assistant", "after_compaction"]
        );
        assert_eq!(
            ids(&history::message_rows(sql, SID, Some(5), None).unwrap()),
            ["latest_compaction", "assistant", "after_compaction"]
        );
        let no_compaction = history::message_rows(sql, SID, None, Some(1)).unwrap();
        assert_eq!(
            ids(&no_compaction),
            [
                "old_user",
                "before_compaction",
                "old_compaction",
                "middle_user",
                "latest_compaction",
                "assistant",
                "after_compaction"
            ]
        );
        assert_eq!(
            no_compaction[3].message_json(),
            json!({"id": "middle_user", "type": "user", "text": "middle"})
        );
        assert_eq!(
            history::message_rows(sql, SID, None, None).unwrap().len(),
            8
        );
        assert!(history::load(sql, "ses_missing").unwrap().is_empty());
    });
}

#[tokio::test]
async fn projected_update_and_epoch_survive_restart_and_blocked_compaction() {
    let (dir, db, events) = setup();
    append(
        &events,
        SID,
        Some(("user1", "user", json!({"text": "first"}))),
    )
    .await;
    let first = prepare(&db, &events, Some("Initial")).await;
    append(&events, SID, Some(("model", "model-switched", json!({})))).await;
    append(
        &events,
        SID,
        Some(("user2", "user", json!({"text": "second"}))),
    )
    .await;
    assert_eq!(prepare(&db, &events, Some("Changed")).await, first);
    let update_id = db.with(|sql| {
        let rows = history::load(sql, SID).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[3].seq, 3);
        assert_eq!(rows[3].message_type, "system");
        assert_eq!(rows[3].data, json!({"text": "Changed"}));
        assert!(rows[3].id.starts_with("msg_"));
        rows[3].id.clone()
    });
    drop(events);
    drop(db);

    let db = TestDb::open(&dir.path().join("history.db"));
    let events = TestEvents::new(db.clone());
    events.project_system_updates();
    assert_eq!(prepare(&db, &events, Some("Changed")).await, first);
    db.with(|sql| assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), 3));
    let compaction = append(
        &events,
        SID,
        Some(("compaction", "compaction", json!({"summary": "s"}))),
    )
    .await;
    assert_eq!(prepare(&db, &events, None).await, first);
    db.with(|sql| {
        assert_eq!(
            ids(&history::load(sql, SID).unwrap()),
            [update_id.as_str(), "compaction"]
        );
        assert_eq!(
            ids(&history::entries_for_runner(sql, SID, first.baseline_seq).unwrap()),
            [update_id.as_str(), "compaction"]
        );
    });
    assert_eq!(
        prepare(&db, &events, Some("Fresh")).await,
        Prepared {
            baseline: "Fresh".into(),
            baseline_seq: compaction
        }
    );
    db.with(|sql| assert_eq!(ids(&history::load(sql, SID).unwrap()), ["compaction"]));
}

#[tokio::test]
async fn event_insert_failure_rolls_back_snapshot_projection_and_sequence_then_retries_once() {
    let (_dir, db, events) = setup();
    let first = prepare(&db, &events, Some("Initial")).await;
    let stored = db.with(|sql| epoch::find(sql, SID)).unwrap().unwrap();
    db.with(|sql| sql.execute_batch("CREATE TRIGGER reject_update BEFORE INSERT ON event BEGIN SELECT RAISE(ABORT, 'injected event failure'); END;")).unwrap();
    let ctx = context(Some("Changed"));
    let error = epoch::prepare(&*db, &events, async { Ok(ctx) }, SID)
        .await
        .unwrap_err();
    assert!(matches!(error, ContextError::Event(_)), "{error}");
    db.with(|sql| {
        assert_eq!(epoch::find(sql, SID).unwrap().unwrap(), stored);
        assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), -1);
        assert!(history::load(sql, SID).unwrap().is_empty());
        assert!(sql.query("SELECT id FROM event", &[]).unwrap().is_empty());
        sql.execute_batch("DROP TRIGGER reject_update").unwrap();
    });
    assert_eq!(prepare(&db, &events, Some("Changed")).await, first);
    assert_eq!(prepare(&db, &events, Some("Changed")).await, first);
    db.with(|sql| {
        assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), 0);
        assert_eq!(history::load(sql, SID).unwrap().len(), 1);
        assert_eq!(sql.query("SELECT id FROM event", &[]).unwrap().len(), 1);
        let snapshot: serde_json::Value =
            serde_json::from_str(&epoch::find(sql, SID).unwrap().unwrap().snapshot).unwrap();
        assert_eq!(snapshot["test/context"]["value"], "Changed");
    });
}

#[tokio::test]
async fn removal_publishes_notice_once_and_keeps_immutable_baseline_after_restart() {
    let (dir, db, events) = setup();
    let initial = prepare(&db, &events, Some("Initial")).await;
    assert_eq!(
        epoch::prepare(&*db, &events, async { Ok(SystemContext::empty()) }, SID)
            .await
            .unwrap(),
        initial
    );
    db.with(|sql| {
        let rows = history::load(sql, SID).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].data, json!({"text": "Context removed"}));
        assert_eq!(epoch::find(sql, SID).unwrap().unwrap().snapshot, "{}");
    });
    drop(events);
    drop(db);
    let db = TestDb::open(&dir.path().join("history.db"));
    let events = TestEvents::new(db.clone());
    assert_eq!(
        epoch::prepare(&*db, &events, async { Ok(SystemContext::empty()) }, SID)
            .await
            .unwrap(),
        initial
    );
    db.with(|sql| assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), 0));
}

#[tokio::test]
async fn existing_epoch_skips_initializer_and_database_failure_cancels_pending_context() {
    let (_dir, db, events) = setup();
    prepare(&db, &events, Some("Initial")).await;
    assert_eq!(
        epoch::initialize(
            &*db,
            async { panic!("existing epoch must not observe context") },
            SID
        )
        .await
        .unwrap(),
        None
    );
    db.with(|sql| {
        sql.execute(
            "DELETE FROM session_context_epoch WHERE session_id = ?",
            &[Value::from(SID)],
        )
    })
    .unwrap();
    db.with(|sql| sql.execute_batch("DROP TABLE session_context_epoch"))
        .unwrap();
    let pending = futures::future::pending::<Result<SystemContext, ContextError>>();
    let result = epoch::prepare(&*db, &events, pending, SID).now_or_never();
    assert!(matches!(result, Some(Err(ContextError::Storage(_)))));
}

#[tokio::test]
async fn deferred_commit_failure_restores_epoch_and_history_before_retry() {
    let (_dir, db, events) = setup();
    let baseline = prepare(&db, &events, Some("Initial")).await;
    let stored = db.with(|sql| epoch::find(sql, SID)).unwrap().unwrap();
    events.project(&epoch::CONTEXT_UPDATED, Arc::new(|sql, _| {
        sql.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        sql.execute("INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ('bad_fk', 'ses_absent', 'system', 1, 0, 0, '{}')", &[])?;
        Ok(())
    }));
    let ctx = context(Some("Changed"));
    assert!(matches!(
        epoch::prepare(&*db, &events, async { Ok(ctx) }, SID).await,
        Err(ContextError::Event(_))
    ));
    db.with(|sql| {
        assert_eq!(epoch::find(sql, SID).unwrap().unwrap(), stored);
        assert!(history::load(sql, SID).unwrap().is_empty());
        assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), -1);
    });
    drop(events);
    let events = TestEvents::new(db.clone());
    events.project_system_updates();
    assert_eq!(prepare(&db, &events, Some("Changed")).await, baseline);
    db.with(|sql| assert_eq!(history::load(sql, SID).unwrap().len(), 1));
}

#[tokio::test]
async fn compaction_at_sequence_zero_replaces_an_empty_event_baseline() {
    let (_dir, db, events) = setup();
    assert_eq!(
        prepare(&db, &events, Some("Initial")).await.baseline_seq,
        -1
    );
    assert_eq!(
        append(&events, SID, Some(("compaction", "compaction", json!({})))).await,
        0
    );
    assert_eq!(
        prepare(&db, &events, Some("Fresh")).await,
        Prepared {
            baseline: "Fresh".into(),
            baseline_seq: 0
        }
    );
    db.with(|sql| {
        assert_eq!(epoch::latest_sequence(sql, SID).unwrap(), 0);
        assert_eq!(ids(&history::load(sql, SID).unwrap()), ["compaction"]);
    });
}
