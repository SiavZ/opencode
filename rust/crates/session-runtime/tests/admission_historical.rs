//! Purpose: D4 blocking proof for the historical message-only retry discrepancy
//! (contracts/phase-04/contract.md "Historical retry discrepancy"). Reproduces the baseline failure
//! (`prompt.admitted` over a visible user message with no inbox row dies with LifecycleConflict)
//! and proves the Rust reconciliation: exact retries synthesize one promoted inbox row at the
//! message seq, including under concurrent retries from two database handles; non-exact retries
//! (other Session, other prompt, non-user message) fail with PromptConflict and write nothing.
//! Scope: integration test over real migrated SQLite. Event store is the real p02 SqlEventStore.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/location.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/tests/support/sql.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::future::join_all
//! |-- location::Ref
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: the message-only state is produced the way old databases have it: the transcript row
//! exists (projected by `session.next.prompted`) and `session_input` has no row for that ID.
//! Contract tests: session-runtime/tests/admission_historical.rs

#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::PublishOptions;
use opencode_schema::location;
use opencode_schema::prompt::Prompt;
use opencode_schema::session_event::{PromptAdmitted, Prompted, Synthetic};
use opencode_schema::values::{AbsolutePath, DateTimeUtc};
use opencode_session_api::operations::SessionService;
use opencode_session_api::{
    Delivery, MessageID, PromptInput, PromptRequest, SessionError, SessionID, SessionResult,
};
use opencode_session_runtime::projection::{ProjectionError, publish, publish_session_event};
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::service::{LocalSessionService, ResolvedProject, SessionLocations};
use opencode_storage_api::{DatabaseExt, Sql, TxOptions, Value};

const SESSION: &str = "ses_prompt_test";

struct NoLocations;

#[async_trait]
impl SessionLocations for NoLocations {
    async fn resolve_project(&self, _: &AbsolutePath) -> SessionResult<ResolvedProject> {
        Err(SessionError::Validation("fixture".into()))
    }
    fn revert(&self, _: &location::Ref) -> SessionResult<Revert> {
        Err(SessionError::Validation("fixture".into()))
    }
}

fn sid() -> SessionID {
    SessionID::new(SESSION).unwrap()
}

fn service(h: &sql::Harness) -> LocalSessionService {
    LocalSessionService::new(
        h.events.clone(),
        Arc::new(sql::RecordingExecution::default()),
        Arc::new(NoLocations),
        "test",
    )
}

fn prompt(text: &str) -> Prompt {
    Prompt {
        text: text.into(),
        files: None,
        agents: None,
    }
}

/// Builds the historical state: a visible user message (projected by `prompted`) with its inbox
/// row removed, i.e. a database migrated from a release that predates `session_input`.
/// Returns (message seq, message time_created).
async fn historical(
    h: &sql::Harness,
    id: &MessageID,
    text: &str,
    delivery: Delivery,
) -> (i64, i64) {
    let timestamp = DateTimeUtc::from_millis(1_700_000_000_123).unwrap();
    let seq = publish_session_event(
        h.events.as_ref(),
        &Prompted {
            timestamp,
            session_id: sid(),
            message_id: id.clone(),
            prompt: prompt(text),
            delivery,
        },
    )
    .await
    .unwrap();
    let id = id.as_str().to_owned();
    h.db.transaction(TxOptions::default(), move |tx: &dyn Sql| {
        tx.execute("DELETE FROM session_input WHERE id = ?", &[id.into()])
    })
    .await
    .unwrap();
    (seq, timestamp.millis())
}

fn retry(id: &MessageID, text: &str, delivery: Option<Delivery>) -> PromptRequest {
    PromptRequest {
        id: Some(id.clone()),
        session_id: sid(),
        prompt: PromptInput::text(text),
        delivery,
        resume: Some(false),
    }
}

async fn inbox_rows(h: &sql::Harness) -> Vec<(String, String, i64, Option<i64>, i64)> {
    h.query(
        "SELECT id, delivery, admitted_seq, promoted_seq, time_created FROM session_input ORDER BY admitted_seq",
        vec![],
    )
    .await
    .iter()
    .map(|row| {
        (
            row.get("id").unwrap(),
            row.get("delivery").unwrap(),
            row.get("admitted_seq").unwrap(),
            row.get("promoted_seq").unwrap(),
            row.get("time_created").unwrap(),
        )
    })
    .collect()
}

#[tokio::test]
async fn reproduces_baseline_admit_failure_on_message_only_history() {
    // Baseline `SessionInput.admit` only consults session_input, so it publishes `prompt.admitted`
    // whose projector finds the visible message and dies with LifecycleConflict.
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let id = MessageID::create();
    historical(&h, &id, "Historical prompt", Delivery::Steer).await;
    assert!(inbox_rows(&h).await.is_empty());

    let error = publish(
        h.events.as_ref(),
        &PromptAdmitted {
            timestamp: DateTimeUtc::now(),
            session_id: sid(),
            message_id: id.clone(),
            prompt: prompt("Historical prompt"),
            delivery: Delivery::Steer,
        },
        PublishOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.projector_source::<ProjectionError>(),
        Some(ProjectionError::LifecycleConflict { id: conflicted }) if conflicted == &id
    ));
    // The failed publish rolled back: no inbox row, no admitted event.
    assert!(inbox_rows(&h).await.is_empty());
    assert_eq!(
        h.count(
            "SELECT count(*) FROM event WHERE type = 'session.next.prompt.admitted.1'",
            vec![]
        )
        .await,
        0
    );
}

#[tokio::test]
async fn exact_retry_of_a_historical_prompt_synthesizes_a_promoted_inbox_row() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let id = MessageID::create();
    let (seq, created) = historical(&h, &id, "Historical prompt", Delivery::Steer).await;
    let events_before = h.count("SELECT count(*) FROM event", vec![]).await;

    let retried = service(&h)
        .prompt(retry(&id, "Historical prompt", None))
        .await
        .unwrap();
    assert_eq!(retried.id, id);
    assert_eq!(retried.prompt.text, "Historical prompt");
    assert_eq!(retried.admitted_seq.get(), seq);
    assert_eq!(retried.promoted_seq.map(|seq| seq.get()), Some(seq));
    assert_eq!(retried.time_created.millis(), created);
    assert_eq!(
        inbox_rows(&h).await,
        vec![(
            id.as_str().to_owned(),
            "steer".into(),
            seq,
            Some(seq),
            created
        )]
    );
    // Reconciliation is local: no new durable event, and the transcript is unchanged.
    assert_eq!(
        h.count("SELECT count(*) FROM event", vec![]).await,
        events_before
    );
    assert_eq!(
        h.count("SELECT count(*) FROM session_message", vec![])
            .await,
        1
    );

    // A second exact retry returns the same row through the normal inbox path.
    let again = service(&h)
        .prompt(retry(&id, "Historical prompt", None))
        .await
        .unwrap();
    assert_eq!(again, retried);
}

#[tokio::test]
async fn exact_retry_of_a_historical_queued_prompt_records_the_requested_delivery() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let id = MessageID::create();
    let (seq, _) = historical(&h, &id, "Historical queued prompt", Delivery::Queue).await;
    let retried = service(&h)
        .prompt(retry(
            &id,
            "Historical queued prompt",
            Some(Delivery::Queue),
        ))
        .await
        .unwrap();
    assert_eq!(retried.delivery, Delivery::Queue);
    assert_eq!(retried.promoted_seq.map(|seq| seq.get()), Some(seq));
}

#[tokio::test]
async fn non_exact_historical_retries_conflict_without_writing() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    h.seed_session("ses_prompt_other").await;
    let id = MessageID::create();
    historical(&h, &id, "Historical prompt", Delivery::Steer).await;
    let svc = service(&h);

    let different_prompt = svc
        .prompt(retry(&id, "Edited prompt", None))
        .await
        .unwrap_err();
    assert!(matches!(
        different_prompt,
        SessionError::PromptConflict { .. }
    ));

    let mut other_session = retry(&id, "Historical prompt", None);
    other_session.session_id = SessionID::new("ses_prompt_other").unwrap();
    let error = svc.prompt(other_session).await.unwrap_err();
    assert!(
        matches!(&error, SessionError::PromptConflict { session_id, .. }
        if session_id.as_str() == "ses_prompt_other")
    );

    // A visible non-user message with the same ID is never reconciled.
    let synthetic = MessageID::create();
    publish_session_event(
        h.events.as_ref(),
        &Synthetic {
            timestamp: DateTimeUtc::now(),
            session_id: sid(),
            message_id: synthetic.clone(),
            text: "Historical prompt".into(),
        },
    )
    .await
    .unwrap();
    let error = svc
        .prompt(retry(&synthetic, "Historical prompt", None))
        .await
        .unwrap_err();
    assert!(matches!(error, SessionError::PromptConflict { .. }));

    assert!(inbox_rows(&h).await.is_empty());
}

#[tokio::test]
async fn historical_reconciliation_respects_attachments_in_prompt_equivalence() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let id = MessageID::create();
    // Historical user row written directly (raw table state of an old database) with a file.
    let seed = id.as_str().to_owned();
    h.db.transaction(TxOptions::default(), move |tx: &dyn Sql| {
        tx.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES (?, ?, 'user', 4, 42, 42, ?)",
            &[
                seed.into(),
                SESSION.into(),
                Value::Text(
                    r#"{"time":{"created":42},"text":"look","files":[{"uri":"file:///project/a.png","mime":"image/png"}]}"#.into(),
                ),
            ],
        )
    })
    .await
    .unwrap();
    let svc = service(&h);
    let error = svc.prompt(retry(&id, "look", None)).await.unwrap_err();
    assert!(matches!(error, SessionError::PromptConflict { .. }));

    let mut exact = retry(&id, "look", None);
    exact.prompt.files = Some(vec![opencode_session_api::prompt::InputFileAttachment {
        uri: "file:///project/a.png".into(),
        name: None,
        description: None,
        source: None,
    }]);
    let admitted = svc.prompt(exact).await.unwrap();
    assert_eq!(admitted.admitted_seq.get(), 4);
    assert_eq!(admitted.time_created.millis(), 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_historical_retries_reconcile_exactly_once() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let id = MessageID::create();
    let (seq, _) = historical(&h, &id, "Concurrent historical", Delivery::Steer).await;
    // Two independent handles (separate connections and projector registries) on the same file.
    let (_, _, events) = sql::connect(&h.path()).await;
    let second = Arc::new(LocalSessionService::new(
        events,
        Arc::new(sql::RecordingExecution::default()),
        Arc::new(NoLocations),
        "test",
    ));
    let first = Arc::new(service(&h));
    let tasks = (0..10).map(|index| {
        let svc = if index % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let req = retry(&id, "Concurrent historical", None);
        tokio::spawn(async move { svc.prompt(req).await })
    });
    let results: Vec<_> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|joined| joined.unwrap().unwrap())
        .collect();
    assert!(results.iter().all(|admitted| admitted == &results[0]));
    assert_eq!(results[0].promoted_seq.map(|seq| seq.get()), Some(seq));
    assert_eq!(inbox_rows(&h).await.len(), 1);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM event WHERE type = 'session.next.prompt.admitted.1'",
            vec![]
        )
        .await,
        0
    );
}
