//! Purpose: transcript projection (message-updater port) and Session store reads: assistant step,
//! text, reasoning and tool lifecycle folding; stale assistant superseded by a new step; shell
//! start/end; message pagination with cursors; list ordering, search and anchors; context history;
//! message lookup scoped to its Session; history paging of durable events.
//! Scope: integration test over real migrated SQLite and the real p02 SqlEventStore.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: expected JSON follows packages/core/src/session/message-updater.ts semantics.
//! Contract tests: session-runtime/tests/admission_projection.rs

#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::{EventStore, PublishOptions};
use opencode_schema::location;
use opencode_schema::values::AbsolutePath;
use opencode_session_api::info::{ListAnchor, ListDirection};
use opencode_session_api::operations::{
    CursorDirection, ListRequest, ListScope, MessagesRequest, Order, SessionService,
};
use opencode_session_api::{MessageID, SessionError, SessionID, SessionResult};
use opencode_session_runtime::projection::definition_of;
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::service::{LocalSessionService, ResolvedProject, SessionLocations};
use opencode_storage_api::{DatabaseExt, Sql, TxOptions};
use serde_json::{Value, json};

const SESSION: &str = "ses_projection";

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

/// Publishes a durable Session event by type name with JSON data (sessionID added).
async fn emit(h: &sql::Harness, name: &str, mut data: Value) {
    data["sessionID"] = json!(SESSION);
    let definition = opencode_schema::session_event::DURABLE_DEFINITIONS
        .iter()
        .rev()
        .find(|d| d.r#type == name)
        .unwrap();
    let durable = definition.durable.unwrap();
    h.store
        .publish(
            &opencode_event_api::Definition::durable(
                definition.r#type,
                durable.version,
                "sessionID",
            ),
            data,
            PublishOptions::default(),
        )
        .await
        .unwrap();
}

async fn all_messages(service: &LocalSessionService) -> Vec<Value> {
    service
        .messages(MessagesRequest {
            session_id: sid(),
            limit: None,
            order: Some(Order::Asc),
            cursor: None,
        })
        .await
        .unwrap()
        .iter()
        .map(|message| serde_json::to_value(message).unwrap())
        .collect()
}

#[tokio::test]
async fn folds_an_assistant_turn_into_one_message() {
    let (h, service) = setup().await;
    let model = json!({"id": "m", "providerID": "p"});
    emit(&h, "session.next.step.started", json!({"timestamp": 10, "assistantMessageID": "msg_a1", "agent": "build", "model": model, "snapshot": "s0"})).await;
    emit(
        &h,
        "session.next.reasoning.started",
        json!({"timestamp": 11, "assistantMessageID": "msg_a1", "reasoningID": "r"}),
    )
    .await;
    emit(&h, "session.next.reasoning.ended", json!({"timestamp": 12, "assistantMessageID": "msg_a1", "reasoningID": "r", "text": "think"})).await;
    emit(
        &h,
        "session.next.text.started",
        json!({"timestamp": 13, "assistantMessageID": "msg_a1", "textID": "t"}),
    )
    .await;
    emit(
        &h,
        "session.next.text.ended",
        json!({"timestamp": 14, "assistantMessageID": "msg_a1", "textID": "t", "text": "hello"}),
    )
    .await;
    emit(
        &h,
        "session.next.tool.input.started",
        json!({"timestamp": 15, "assistantMessageID": "msg_a1", "callID": "c", "name": "read"}),
    )
    .await;
    emit(&h, "session.next.tool.input.ended", json!({"timestamp": 16, "assistantMessageID": "msg_a1", "callID": "c", "text": "{\"p\":1}"})).await;
    emit(&h, "session.next.tool.called", json!({"timestamp": 17, "assistantMessageID": "msg_a1", "callID": "c", "tool": "read", "input": {"p": 1}, "provider": {"executed": false}})).await;
    emit(&h, "session.next.tool.success", json!({"timestamp": 18, "assistantMessageID": "msg_a1", "callID": "c", "structured": {"ok": true}, "content": [], "provider": {"executed": true}})).await;
    emit(&h, "session.next.step.ended", json!({"timestamp": 19, "assistantMessageID": "msg_a1", "finish": "stop", "cost": 0.5, "tokens": {"input": 1, "output": 2, "reasoning": 0, "cache": {"read": 0, "write": 0}}, "snapshot": "s1"})).await;

    let messages = all_messages(&service).await;
    assert_eq!(messages.len(), 1);
    let assistant = &messages[0];
    assert_eq!(assistant["time"], json!({"created": 10, "completed": 19}));
    assert_eq!(assistant["finish"], "stop");
    assert_eq!(assistant["snapshot"], json!({"start": "s0", "end": "s1"}));
    assert_eq!(assistant["content"][0]["text"], "think");
    assert_eq!(
        assistant["content"][0]["time"],
        json!({"created": 11, "completed": 12})
    );
    assert_eq!(
        assistant["content"][1],
        json!({"type": "text", "id": "t", "text": "hello"})
    );
    let tool = &assistant["content"][2];
    assert_eq!(tool["state"]["status"], "completed");
    assert_eq!(tool["state"]["input"], json!({"p": 1}));
    assert_eq!(tool["state"]["outputPaths"], json!([]));
    assert_eq!(tool["provider"], json!({"executed": true}));
    assert_eq!(
        tool["time"],
        json!({"created": 15, "ran": 17, "completed": 18})
    );
}

#[tokio::test]
async fn new_step_completes_stale_assistant_and_failed_tools_keep_running_state() {
    let (h, service) = setup().await;
    let model = json!({"id": "m", "providerID": "p"});
    emit(
        &h,
        "session.next.step.started",
        json!({"timestamp": 1, "assistantMessageID": "msg_a1", "agent": "build", "model": model}),
    )
    .await;
    emit(
        &h,
        "session.next.tool.input.started",
        json!({"timestamp": 2, "assistantMessageID": "msg_a1", "callID": "c", "name": "bash"}),
    )
    .await;
    emit(&h, "session.next.tool.failed", json!({"timestamp": 3, "assistantMessageID": "msg_a1", "callID": "c", "error": {"type": "unknown", "message": "boom"}, "provider": {"executed": false}})).await;
    emit(
        &h,
        "session.next.step.started",
        json!({"timestamp": 4, "assistantMessageID": "msg_a2", "agent": "build", "model": model}),
    )
    .await;
    emit(
        &h,
        "session.next.shell.started",
        json!({"timestamp": 5, "messageID": "msg_s1", "callID": "sh", "command": "ls"}),
    )
    .await;
    emit(
        &h,
        "session.next.shell.ended",
        json!({"timestamp": 6, "callID": "sh", "output": "a"}),
    )
    .await;

    let messages = all_messages(&service).await;
    assert_eq!(messages[0]["time"], json!({"created": 1, "completed": 4}));
    let tool = &messages[0]["content"][0];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(tool["state"]["input"], json!({}));
    assert_eq!(tool["state"]["error"]["message"], "boom");
    assert_eq!(messages[1]["time"], json!({"created": 4}));
    assert_eq!(messages[2]["output"], "a");
    assert_eq!(messages[2]["time"], json!({"created": 5, "completed": 6}));
}

async fn synthetic(h: &sql::Harness, count: usize) -> Vec<MessageID> {
    let mut ids = Vec::new();
    for index in 0..count {
        let id = MessageID::create();
        emit(
            h,
            "session.next.synthetic",
            json!({"timestamp": index, "messageID": id, "text": format!("m{index}")}),
        )
        .await;
        ids.push(id);
    }
    ids
}

fn page(
    cursor: Option<(MessageID, CursorDirection)>,
    order: Option<Order>,
    limit: Option<u32>,
) -> MessagesRequest {
    MessagesRequest {
        session_id: sid(),
        limit,
        order,
        cursor,
    }
}

async fn texts(service: &LocalSessionService, request: MessagesRequest) -> Vec<String> {
    service
        .messages(request)
        .await
        .unwrap()
        .iter()
        .map(|m| {
            serde_json::to_value(m).unwrap()["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn messages_paginate_by_seq_with_cursors() {
    let (h, service) = setup().await;
    let ids = synthetic(&h, 5).await;
    assert_eq!(
        texts(&service, page(None, None, Some(2))).await,
        ["m4", "m3"]
    );
    assert_eq!(
        texts(
            &service,
            page(Some((ids[3].clone(), CursorDirection::Next)), None, Some(2))
        )
        .await,
        ["m2", "m1"]
    );
    // `previous` flips the order, then reverses so the page keeps the requested order.
    assert_eq!(
        texts(
            &service,
            page(
                Some((ids[1].clone(), CursorDirection::Previous)),
                None,
                Some(2)
            )
        )
        .await,
        ["m3", "m2"]
    );
    assert_eq!(
        texts(
            &service,
            page(
                Some((ids[2].clone(), CursorDirection::Next)),
                Some(Order::Asc),
                None
            )
        )
        .await,
        ["m3", "m4"]
    );
    assert!(
        texts(
            &service,
            page(
                Some((MessageID::create(), CursorDirection::Next)),
                None,
                None
            )
        )
        .await
        .is_empty()
    );
    let missing = SessionID::new("ses_missing").unwrap();
    let mut request = page(None, None, None);
    request.session_id = missing;
    assert_eq!(
        service.messages(request).await.unwrap_err().tag(),
        "Session.NotFoundError"
    );

    // `message` is scoped to its Session; `context` returns visible history.
    assert!(service.message(&sid(), &ids[0]).await.unwrap().is_some());
    h.seed_session("ses_other").await;
    assert!(
        service
            .message(&SessionID::new("ses_other").unwrap(), &ids[0])
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(service.context(&sid()).await.unwrap().len(), 5);

    let history = service.history(&sid(), Some(1), 2).await.unwrap();
    assert_eq!(history.events.len(), 2);
    assert!(history.has_more);
    assert_eq!(history.events[0]["durable"]["seq"], 2);
    assert_eq!(history.events[0]["type"], "session.next.synthetic");
}

#[tokio::test]
async fn corrupt_message_rows_surface_message_decode_error() {
    let (h, service) = setup().await;
    h.db.transaction(TxOptions::default(), |tx: &dyn Sql| {
        tx.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ('msg_bad', ?, 'assistant', 0, 0, 0, '{}')",
            &[SESSION.into()],
        )
    })
    .await
    .unwrap();
    let error = service.messages(page(None, None, None)).await.unwrap_err();
    assert_eq!(error.tag(), "Session.MessageDecodeError");
}

#[tokio::test]
async fn list_orders_searches_and_anchors_sessions() {
    let (h, service) = setup().await;
    h.db.transaction(TxOptions::default(), |tx: &dyn Sql| {
        for (id, time, title, directory) in [
            ("ses_a", 10, "alpha", "/project"),
            ("ses_b", 20, "beta", "/project/sub"),
            ("ses_c", 20, "gamma alpha", "/project"),
        ] {
            tx.execute(
                "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated, model) VALUES (?, 'global', 's', ?, ?, 'v', ?, ?, '{\"id\":\"m\",\"providerID\":\"p\"}')",
                &[id.into(), directory.into(), title.into(), time.into(), time.into()],
            )?;
        }
        Ok::<_, opencode_storage_api::StorageError>(())
    })
    .await
    .unwrap();
    let ids = |infos: Vec<opencode_session_api::SessionInfo>| -> Vec<String> {
        infos
            .into_iter()
            .map(|info| info.id.as_str().to_owned())
            .collect()
    };
    let all = service.list(ListRequest::default()).await.unwrap();
    assert_eq!(
        all[0]
            .model
            .as_ref()
            .unwrap()
            .variant
            .as_ref()
            .unwrap()
            .as_str(),
        "default"
    );
    assert_eq!(ids(all), ["ses_c", "ses_b", "ses_a", SESSION]);
    let search = ListRequest {
        search: Some("alpha".into()),
        order: Some(Order::Asc),
        ..ListRequest::default()
    };
    assert_eq!(ids(service.list(search).await.unwrap()), ["ses_a", "ses_c"]);
    let directory = ListRequest {
        scope: ListScope::Directory("/project/sub".into()),
        ..ListRequest::default()
    };
    assert_eq!(ids(service.list(directory).await.unwrap()), ["ses_b"]);
    let next = ListRequest {
        anchor: Some(ListAnchor {
            id: SessionID::new("ses_c").unwrap(),
            time: 20.into(),
            direction: ListDirection::Next,
        }),
        limit: Some(2),
        ..ListRequest::default()
    };
    assert_eq!(ids(service.list(next).await.unwrap()), ["ses_b", "ses_a"]);
    let previous = ListRequest {
        anchor: Some(ListAnchor {
            id: SessionID::new("ses_a").unwrap(),
            time: 10.into(),
            direction: ListDirection::Previous,
        }),
        limit: Some(1),
        ..ListRequest::default()
    };
    assert_eq!(ids(service.list(previous).await.unwrap()), ["ses_b"]);
    let _ = definition_of::<opencode_schema::session_event::Synthetic>();
}
