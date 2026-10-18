//! Purpose: SessionService prompt admission contract through real migrated SQLite: durable admission
//! before transcript promotion, resume:false admit-only, default and explicit wake, MIME resolution,
//! exact retry, conflicting reuse, global message ID uniqueness, visible-history conflict, concurrent
//! exact retries, create adoption (sequential and concurrent), switch model/agent, unavailable ops.
//! Scope: integration test. SQLite, migrations, the p02
//! SqlEventStore, projectors, admission and service are real; execution and Location ports are fixtures.
//! Uses:
//! rust/crates/
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/service.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/core/test/session-prompt.test.ts cases by name.
//! Contract tests: session-runtime/tests/admission.rs

#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;

use async_trait::async_trait;
use opencode_schema::location;
use opencode_schema::values::AbsolutePath;
use opencode_schema::{ModelID, ProjectID, ProviderID};
use opencode_session_api::operations::{CreateRequest, MessagesRequest, SessionService};
use opencode_session_api::prompt::InputFileAttachment;
use opencode_session_api::{
    Delivery, MessageID, ModelRef, PromptInput, PromptRequest, SessionError, SessionID,
    SessionResult,
};
use opencode_session_runtime::inbox;
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::service::{LocalSessionService, ResolvedProject, SessionLocations};
use opencode_storage_api::DatabaseExt;

const SESSION: &str = "ses_prompt_test";

/// Fixture Location ports: every directory resolves to the global project at `/project`.
struct FixtureLocations;

#[async_trait]
impl SessionLocations for FixtureLocations {
    async fn resolve_project(&self, _: &AbsolutePath) -> SessionResult<ResolvedProject> {
        Ok(ResolvedProject {
            id: ProjectID::global(),
            directory: AbsolutePath::new("/project"),
            vcs: None,
        })
    }
    fn revert(&self, _: &location::Ref) -> SessionResult<Revert> {
        Err(SessionError::Validation("fixture has no revert".into()))
    }
}

struct Ctx {
    h: sql::Harness,
    exec: Arc<sql::RecordingExecution>,
    service: LocalSessionService,
}

async fn setup() -> Ctx {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let exec = Arc::new(sql::RecordingExecution::default());
    let service = LocalSessionService::new(
        h.events.clone(),
        exec.clone(),
        Arc::new(FixtureLocations),
        "test",
    );
    Ctx { h, exec, service }
}

fn sid() -> SessionID {
    SessionID::new(SESSION).unwrap()
}

fn request(id: Option<&MessageID>, text: &str) -> PromptRequest {
    PromptRequest {
        id: id.cloned(),
        session_id: sid(),
        prompt: PromptInput::text(text),
        delivery: None,
        resume: Some(false),
    }
}

async fn admitted(ctx: &Ctx, id: &MessageID) -> Option<opencode_session_api::Admitted> {
    let id = id.as_str().to_owned();
    ctx.h
        .db
        .read(move |tx: &dyn opencode_storage_api::Sql| inbox::find(tx, &id))
        .await
        .unwrap()
}

async fn messages(ctx: &Ctx) -> Vec<opencode_session_api::Message> {
    ctx.service
        .messages(MessagesRequest {
            session_id: sid(),
            limit: None,
            order: None,
            cursor: None,
        })
        .await
        .unwrap()
}

fn is_conflict(error: &SessionError, session: &str, id: &MessageID) -> bool {
    matches!(error, SessionError::PromptConflict { session_id, message_id }
        if session_id.as_str() == session && message_id == id)
        && error.tag() == "Session.PromptConflictError"
}

#[tokio::test]
async fn durably_admits_one_user_message_before_transcript_promotion() {
    let ctx = setup().await;
    let message = ctx
        .service
        .prompt(request(None, "Fix the failing tests"))
        .await
        .unwrap();
    assert_eq!(message.prompt.text, "Fix the failing tests");
    assert!(messages(&ctx).await.is_empty());
    let stored = admitted(&ctx, &message.id).await.unwrap();
    assert_eq!(stored.session_id, sid());
    assert_eq!(stored.delivery, Delivery::Steer);
    assert_eq!(stored.promoted_seq, None);
    assert_eq!(stored, message);
    assert!(ctx.exec.wakes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn resolves_attachment_mime_before_admission() {
    let ctx = setup().await;
    let mut req = request(None, "Inspect this image");
    req.prompt.files = Some(vec![InputFileAttachment {
        uri: "data:image/png;base64,aGVsbG8=".into(),
        name: Some("image.png".into()),
        description: None,
        source: None,
    }]);
    let message = ctx.service.prompt(req).await.unwrap();
    assert_eq!(
        serde_json::to_string(&message.prompt.files).unwrap(),
        r#"[{"uri":"data:image/png;base64,aGVsbG8=","mime":"image/png","name":"image.png"}]"#
    );
    assert_eq!(
        admitted(&ctx, &message.id).await.unwrap().prompt.files,
        message.prompt.files
    );
}

#[tokio::test]
async fn records_distinct_messages_when_the_id_is_omitted() {
    let ctx = setup().await;
    let first = ctx.service.prompt(request(None, "one")).await.unwrap();
    let second = ctx.service.prompt(request(None, "one")).await.unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.admitted_seq.get(), first.admitted_seq.get() + 1);
    assert_eq!(
        ctx.h
            .count("SELECT count(*) FROM session_input", vec![])
            .await,
        2
    );
}

#[tokio::test]
async fn returns_the_original_recorded_message_when_the_id_is_retried() {
    let ctx = setup().await;
    let id = MessageID::create();
    let first = ctx
        .service
        .prompt(request(Some(&id), "same"))
        .await
        .unwrap();
    let retried = ctx
        .service
        .prompt(request(Some(&id), "same"))
        .await
        .unwrap();
    assert_eq!(first, retried);
    assert_eq!(
        ctx.h
            .count(
                "SELECT count(*) FROM event WHERE type = 'session.next.prompt.admitted.1'",
                vec![]
            )
            .await,
        1
    );
}

#[tokio::test]
async fn wakes_execution_when_an_exact_prompt_retry_recovers_a_committed_message() {
    let ctx = setup().await;
    let id = MessageID::create();
    ctx.service
        .prompt(request(Some(&id), "recover"))
        .await
        .unwrap();
    let mut retry = request(Some(&id), "recover");
    retry.resume = None;
    ctx.service.prompt(retry).await.unwrap();
    assert_eq!(*ctx.exec.wakes.lock().unwrap(), vec![sid()]);
}

#[tokio::test]
async fn rejects_reuse_of_one_id_with_a_different_prompt() {
    let ctx = setup().await;
    let id = MessageID::create();
    ctx.service
        .prompt(request(Some(&id), "first"))
        .await
        .unwrap();
    let error = ctx
        .service
        .prompt(request(Some(&id), "second"))
        .await
        .unwrap_err();
    assert!(is_conflict(&error, SESSION, &id));
    assert_eq!(admitted(&ctx, &id).await.unwrap().prompt.text, "first");
}

#[tokio::test]
async fn rejects_reuse_of_one_id_with_a_different_delivery_mode() {
    let ctx = setup().await;
    let id = MessageID::create();
    ctx.service
        .prompt(request(Some(&id), "mode"))
        .await
        .unwrap();
    let mut queued = request(Some(&id), "mode");
    queued.delivery = Some(Delivery::Queue);
    let error = ctx.service.prompt(queued).await.unwrap_err();
    assert!(is_conflict(&error, SESSION, &id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn returns_one_recorded_message_to_concurrent_exact_retries() {
    let ctx = setup().await;
    let id = MessageID::create();
    let service = Arc::new(ctx.service.clone());
    let attempts = (0..8).map(|_| {
        let service = service.clone();
        let req = request(Some(&id), "concurrent");
        tokio::spawn(async move { service.prompt(req).await })
    });
    let results: Vec<_> = futures::future::join_all(attempts)
        .await
        .into_iter()
        .map(|joined| joined.unwrap().unwrap())
        .collect();
    assert!(results.iter().all(|admitted| admitted == &results[0]));
    assert_eq!(
        ctx.h
            .count("SELECT count(*) FROM session_input", vec![])
            .await,
        1
    );
}

#[tokio::test]
async fn rejects_reuse_of_one_globally_unique_message_id_across_sessions() {
    let ctx = setup().await;
    ctx.h.seed_session("ses_prompt_other").await;
    let id = MessageID::create();
    ctx.service
        .prompt(request(Some(&id), "Fix the failing tests"))
        .await
        .unwrap();
    let mut other = request(Some(&id), "Fix the failing tests");
    other.session_id = SessionID::new("ses_prompt_other").unwrap();
    let error = ctx.service.prompt(other).await.unwrap_err();
    assert!(is_conflict(&error, "ses_prompt_other", &id));
}

#[tokio::test]
async fn rejects_a_prompt_id_already_used_by_visible_session_history() {
    let ctx = setup().await;
    let id = MessageID::create();
    opencode_session_runtime::projection::publish_session_event(
        ctx.h.events.as_ref(),
        &opencode_schema::session_event::Synthetic {
            timestamp: opencode_schema::values::DateTimeUtc::now(),
            session_id: sid(),
            message_id: id.clone(),
            text: "Existing history".into(),
        },
    )
    .await
    .unwrap();
    let error = ctx
        .service
        .prompt(request(Some(&id), "Conflicting prompt"))
        .await
        .unwrap_err();
    assert!(is_conflict(&error, SESSION, &id));
    assert!(admitted(&ctx, &id).await.is_none());
}

#[tokio::test]
async fn wake_follows_resume_flag_and_unknown_session_is_not_found() {
    let ctx = setup().await;
    let mut default = request(None, "default");
    default.resume = None;
    ctx.service.prompt(default).await.unwrap();
    let mut explicit = request(None, "explicit");
    explicit.resume = Some(true);
    ctx.service.prompt(explicit).await.unwrap();
    ctx.service
        .prompt(request(None, "admit only"))
        .await
        .unwrap();
    assert_eq!(*ctx.exec.wakes.lock().unwrap(), vec![sid(), sid()]);

    let mut missing = request(None, "missing");
    missing.session_id = SessionID::new("ses_missing").unwrap();
    let error = ctx.service.prompt(missing).await.unwrap_err();
    assert_eq!(error.tag(), "Session.NotFoundError");
    assert!(
        ctx.service
            .resume(&SessionID::new("ses_missing").unwrap())
            .await
            .is_err()
    );
    ctx.service.resume(&sid()).await.unwrap();
    ctx.service
        .interrupt(&SessionID::new("ses_missing").unwrap())
        .await;
    assert_eq!(ctx.exec.resumes.lock().unwrap().len(), 1);
    assert_eq!(ctx.exec.interrupts.lock().unwrap().len(), 1);
}

fn create_request(id: &str) -> CreateRequest {
    CreateRequest {
        id: Some(SessionID::new(id).unwrap()),
        agent: None,
        model: None,
        location: location::Ref {
            directory: AbsolutePath::new("/project/packages/core"),
            workspace_id: None,
        },
    }
}

#[tokio::test]
async fn create_records_once_and_adopts_existing_ids() {
    let ctx = setup().await;
    let created = ctx
        .service
        .create(create_request("ses_created"))
        .await
        .unwrap();
    assert_eq!(created.project_id, ProjectID::global());
    assert_eq!(
        created.subpath.as_ref().map(|p| p.as_str()),
        Some("packages/core")
    );
    assert!(created.title.starts_with("New session - "));
    let again = ctx
        .service
        .create(create_request("ses_created"))
        .await
        .unwrap();
    assert_eq!(created, again);
    assert_eq!(
        ctx.h
            .count(
                "SELECT count(*) FROM event WHERE type = 'session.created.1'",
                vec![]
            )
            .await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_adopts_the_winning_session() {
    let ctx = setup().await;
    // A second handle on the same file: the store pre-check races past and the projector decides.
    let (_, _, events) = sql::connect(&ctx.h.path()).await;
    let other = Arc::new(LocalSessionService::new(
        events,
        ctx.exec.clone(),
        Arc::new(FixtureLocations),
        "test",
    ));
    let mine = Arc::new(ctx.service.clone());
    let tasks = (0..6).map(|index| {
        let service: Arc<LocalSessionService> = if index % 2 == 0 {
            mine.clone()
        } else {
            other.clone()
        };
        tokio::spawn(async move { service.create(create_request("ses_race")).await })
    });
    let results: Vec<_> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|joined| joined.unwrap().unwrap())
        .collect();
    assert!(results.iter().all(|info| info == &results[0]));
    assert_eq!(
        ctx.h
            .count("SELECT count(*) FROM session WHERE id = 'ses_race'", vec![])
            .await,
        1
    );
}

#[tokio::test]
async fn switch_model_and_agent_project_messages_and_session_columns() {
    let ctx = setup().await;
    let model = ModelRef {
        id: ModelID::new("gpt"),
        provider_id: ProviderID::new("openai"),
        variant: None,
    };
    ctx.service
        .switch_model(&sid(), model.clone())
        .await
        .unwrap();
    // Same selection (missing variant equals "default") is a no-op.
    ctx.service.switch_model(&sid(), model).await.unwrap();
    ctx.service.switch_agent(&sid(), "plan").await.unwrap();
    let info = ctx.service.get(&sid()).await.unwrap();
    assert_eq!(info.agent.as_ref().map(|a| a.as_str()), Some("plan"));
    assert_eq!(
        info.model
            .as_ref()
            .and_then(|m| m.variant.as_ref())
            .map(|v| v.as_str()),
        Some("default")
    );
    let kinds: Vec<_> = messages(&ctx)
        .await
        .iter()
        .map(|message| opencode_session_api::message::kind(message))
        .collect();
    assert_eq!(kinds, ["agent-switched", "model-switched"]);
}

#[tokio::test]
async fn unavailable_operations_report_their_tag() {
    let ctx = setup().await;
    let shell = ctx.service.shell(&sid(), "ls").await.unwrap_err();
    assert_eq!(shell.to_string(), "Session operation unavailable: shell");
    assert_eq!(
        ctx.service
            .skill(&sid(), "x")
            .await
            .unwrap_err()
            .to_string(),
        "Session operation unavailable: skill"
    );
    assert_eq!(
        ctx.service.compact(&sid()).await.unwrap_err().to_string(),
        "Session operation unavailable: compact"
    );
    let missing = SessionID::new("ses_missing").unwrap();
    assert_eq!(
        ctx.service.wait(&missing).await.unwrap_err().tag(),
        "Session.NotFoundError"
    );
}
