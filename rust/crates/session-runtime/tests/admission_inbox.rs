//! Purpose: inbox seam contract for the runner (`has_pending`, `promote_steers` with cutoff,
//! `promote_next_queued`) and the `prompted` projection: promotion once under concurrent attempts,
//! cutoff batching, FIFO queue promotion, promoted transcript rows, and revert-commit pruning.
//! Scope: integration test over real migrated SQLite; event store is the real p02 SqlEventStore.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/service.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/core/test/session-prompt.test.ts promotion cases.
//! Contract tests: session-runtime/tests/admission_inbox.rs

#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;

use async_trait::async_trait;
use opencode_schema::location;
use opencode_schema::session_event::RevertCommitted;
use opencode_schema::values::{AbsolutePath, DateTimeUtc};
use opencode_session_api::operations::{MessagesRequest, SessionService};
use opencode_session_api::{
    Delivery, Message, PromptInput, PromptRequest, SessionError, SessionID, SessionResult,
};
use opencode_session_runtime::inbox;
use opencode_session_runtime::projection::publish_session_event;
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::service::{LocalSessionService, ResolvedProject, SessionLocations};
use opencode_storage_api::{DatabaseExt, Sql};

const SESSION: &str = "ses_inbox_test";

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

async fn setup() -> (sql::Harness, LocalSessionService) {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let service = LocalSessionService::new(
        h.events.clone(),
        Arc::new(sql::RecordingExecution::default()),
        Arc::new(NoLocations),
        "test",
    );
    (h, service)
}

async fn admit(
    service: &LocalSessionService,
    text: &str,
    delivery: Delivery,
) -> opencode_session_api::Admitted {
    service
        .prompt(PromptRequest {
            id: None,
            session_id: sid(),
            prompt: PromptInput::text(text),
            delivery: Some(delivery),
            resume: Some(false),
        })
        .await
        .unwrap()
}

async fn pending(h: &sql::Harness, delivery: Delivery) -> bool {
    h.db.read(move |tx: &dyn Sql| {
        Ok::<_, opencode_storage_api::StorageError>(inbox::has_pending(tx, &sid(), delivery))
    })
    .await
    .unwrap()
    .unwrap()
}

async fn texts(service: &LocalSessionService) -> Vec<String> {
    service
        .messages(MessagesRequest {
            session_id: sid(),
            limit: None,
            order: Some(opencode_session_api::operations::Order::Asc),
            cursor: None,
        })
        .await
        .unwrap()
        .into_iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text),
            _ => None,
        })
        .collect()
}

async fn promoted(h: &sql::Harness, id: &opencode_session_api::MessageID) -> Option<i64> {
    let id = id.as_str().to_owned();
    h.db.read(move |tx: &dyn Sql| inbox::find(tx, &id))
        .await
        .unwrap()
        .and_then(|row| row.promoted_seq.map(|seq| seq.get()))
}

#[tokio::test]
async fn promotes_steers_only_through_the_captured_inbox_cutoff() {
    let (h, service) = setup().await;
    let first = admit(&service, "Before cutoff", Delivery::Steer).await;
    let cutoff = first.admitted_seq.get();
    let second = admit(&service, "After cutoff", Delivery::Steer).await;
    assert!(pending(&h, Delivery::Steer).await);

    let count = inbox::promote_steers(h.events.as_ref(), &sid(), cutoff)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(promoted(&h, &first.id).await.is_some());
    assert!(promoted(&h, &second.id).await.is_none());
    assert_eq!(texts(&service).await, ["Before cutoff"]);

    let rest = inbox::promote_steers(h.events.as_ref(), &sid(), i64::MAX)
        .await
        .unwrap();
    assert_eq!(rest, 1);
    assert!(!pending(&h, Delivery::Steer).await);
    assert_eq!(texts(&service).await, ["Before cutoff", "After cutoff"]);
}

#[tokio::test]
async fn queue_promotes_one_input_in_fifo_order_and_ignores_steers() {
    let (h, service) = setup().await;
    admit(&service, "steer", Delivery::Steer).await;
    let q1 = admit(&service, "queued one", Delivery::Queue).await;
    let q2 = admit(&service, "queued two", Delivery::Queue).await;
    assert!(pending(&h, Delivery::Queue).await);

    assert!(
        inbox::promote_next_queued(h.events.as_ref(), &sid())
            .await
            .unwrap()
    );
    assert!(promoted(&h, &q1.id).await.is_some());
    assert!(promoted(&h, &q2.id).await.is_none());
    assert!(
        inbox::promote_next_queued(h.events.as_ref(), &sid())
            .await
            .unwrap()
    );
    assert!(
        !inbox::promote_next_queued(h.events.as_ref(), &sid())
            .await
            .unwrap()
    );
    assert!(!pending(&h, Delivery::Queue).await);
    assert!(pending(&h, Delivery::Steer).await);
    assert_eq!(texts(&service).await, ["queued one", "queued two"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn promotes_one_message_once_under_concurrent_promotion_attempts() {
    let (h, service) = setup().await;
    let admitted = admit(&service, "Promote once", Delivery::Steer).await;
    let cutoff = admitted.admitted_seq.get();
    let events = h.events.clone();
    let tasks = (0..6).map(|_| {
        let events = events.clone();
        tokio::spawn(async move { inbox::promote_steers(events.as_ref(), &sid(), cutoff).await })
    });
    for joined in futures::future::join_all(tasks).await {
        joined.unwrap().unwrap();
    }
    assert_eq!(texts(&service).await, ["Promote once"]);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM event WHERE type = 'session.next.prompted.1'",
            vec![]
        )
        .await,
        1
    );
    // The promoted user message carries the admission time and the promotion seq.
    let row = &h
        .query(
            "SELECT seq, time_created FROM session_message WHERE id = ?",
            vec![admitted.id.as_str().into()],
        )
        .await[0];
    assert_eq!(
        row.get::<i64>("time_created").unwrap(),
        admitted.time_created.millis()
    );
    assert_eq!(
        Some(row.get::<i64>("seq").unwrap()),
        promoted(&h, &admitted.id).await
    );
}

#[tokio::test]
async fn revert_commit_prunes_later_messages_and_inbox_rows() {
    let (h, service) = setup().await;
    let first = admit(&service, "keep", Delivery::Steer).await;
    inbox::promote_steers(h.events.as_ref(), &sid(), i64::MAX)
        .await
        .unwrap();
    let second = admit(&service, "drop", Delivery::Steer).await;
    inbox::promote_steers(h.events.as_ref(), &sid(), i64::MAX)
        .await
        .unwrap();
    let pending_row = admit(&service, "pending", Delivery::Queue).await;
    publish_session_event(
        h.events.as_ref(),
        &RevertCommitted {
            timestamp: DateTimeUtc::now(),
            session_id: sid(),
            message_id: first.id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(texts(&service).await, ["keep"]);
    assert!(promoted(&h, &first.id).await.is_some());
    for gone in [&second.id, &pending_row.id] {
        let id = gone.as_str().to_owned();
        let row =
            h.db.read(move |tx: &dyn Sql| inbox::find(tx, &id))
                .await
                .unwrap();
        assert!(row.is_none());
    }
}
