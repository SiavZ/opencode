//! Purpose: current Session and message handlers (`packages/server/src/handlers/session.ts`,
//! `handlers/message.ts`) over the Phase 04 `SessionService`, including domain-error mapping to
//! the tagged envelopes and cursor paging.
//! Scope: per-request.
//! Uses:
//! rust/crates/
//! |-- schema/src/session.rs
//! |-- server/src/handlers/cursor.rs
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/middleware/errors.rs
//! `-- session-api/src/operations.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/mod.rs
//! Invariants: list default limit 50, history default limit 50, messages default limit 50.
//! `Session.NotFoundError` -> SessionNotFoundError, `PromptConflictError` -> ConflictError,
//! `OperationUnavailableError` -> ServiceUnavailableError `session.<op>`, decode failures ->
//! UnknownError with an `err_` reference. `session.events` is owned by p06.server-streams.
//! Contract tests: server/tests/http_session.rs

use std::sync::Arc;

use axum::response::Response;
use opencode_protocol::errors::ApiError;
use opencode_schema::session::{Direction, ListAnchor};
use opencode_schema::values::Finite;
use opencode_session_api::admission::PromptRequest;
use opencode_session_api::error::SessionError;
use opencode_session_api::ids::{
    AbsolutePath, MessageID, ProjectID, RelativePath, SessionID, WorkspaceID,
};
use opencode_session_api::operations::{
    CreateRequest, CursorDirection, ListRequest, ListScope, MessagesRequest, Order,
    RevertStageRequest, SessionService,
};
use serde_json::{Map, Value, json};

use crate::handlers::cursor::{MessageCursor, SessionsCursor};
use crate::handlers::dispatch::typed;
use crate::handlers::{Ctx, no_content, ok};
use crate::middleware::errors::{DEFECT_MESSAGE, HttpError, defect_ref};
use crate::middleware::legacy_routing::current_dir;

const DEFAULT_SESSIONS_LIMIT: u32 = 50;
const DEFAULT_HISTORY_LIMIT: usize = 50;
const DEFAULT_MESSAGES_LIMIT: u32 = 50;

pub async fn handle(ctx: Ctx) -> Result<Response, HttpError> {
    let name = ctx.endpoint.name.as_str();
    if name == "session.events" {
        return Err(HttpError::not_yet_ported(name));
    }
    let Some(session) = ctx.state.services.session.clone() else {
        return Err(HttpError::not_yet_ported(name));
    };
    match name {
        "session.list" => list(&ctx, session).await,
        "session.create" => create(&ctx, session).await,
        "session.active" => {
            let data: Map<String, Value> = session
                .active()
                .await
                .into_iter()
                .map(|id| (id.into_inner(), json!({ "type": "running" })))
                .collect();
            Ok(ok(&json!({ "data": data })))
        }
        "session.get" => {
            let info = session.get(&sid(&ctx)?).await.map_err(map)?;
            Ok(ok(&json!({ "data": info })))
        }
        "session.switchAgent" => {
            let agent = payload_str(&ctx, "agent");
            session
                .switch_agent(&sid(&ctx)?, &agent)
                .await
                .map_err(map)?;
            Ok(no_content())
        }
        "session.switchModel" => {
            let model = typed(true, "Payload", payload_field(&ctx, "model"))?;
            session
                .switch_model(&sid(&ctx)?, model)
                .await
                .map_err(map)?;
            Ok(no_content())
        }
        "session.prompt" => prompt(&ctx, session).await,
        "session.compact" => {
            session.compact(&sid(&ctx)?).await.map_err(map)?;
            Ok(no_content())
        }
        "session.wait" => {
            session.wait(&sid(&ctx)?).await.map_err(map)?;
            Ok(no_content())
        }
        "session.revert.stage" => {
            let request = RevertStageRequest {
                session_id: sid(&ctx)?,
                message_id: typed(true, "Payload", payload_field(&ctx, "messageID"))?,
                files: payload_field(&ctx, "files").as_bool(),
            };
            let state = session.revert_stage(request).await.map_err(map)?;
            Ok(ok(&json!({ "data": state })))
        }
        "session.revert.clear" => {
            session.revert_clear(&sid(&ctx)?).await.map_err(map)?;
            Ok(no_content())
        }
        "session.revert.commit" => {
            session.revert_commit(&sid(&ctx)?).await.map_err(map)?;
            Ok(no_content())
        }
        "session.context" => {
            let data = session.context(&sid(&ctx)?).await.map_err(map)?;
            Ok(ok(&json!({ "data": data })))
        }
        "session.history" => {
            let after = ctx.query_num("after").map(|n| n as i64);
            let limit = ctx
                .query_num("limit")
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_HISTORY_LIMIT);
            let page = session
                .history(&sid(&ctx)?, after, limit)
                .await
                .map_err(map)?;
            Ok(ok(
                &json!({ "data": page.events, "hasMore": page.has_more }),
            ))
        }
        "session.interrupt" => {
            session.interrupt(&sid(&ctx)?).await;
            Ok(no_content())
        }
        "session.message" => {
            let session_id = sid(&ctx)?;
            let raw = ctx.param("messageID");
            let message_id: MessageID = typed(true, "Params", Value::String(raw.clone()))?;
            match session
                .message(&session_id, &message_id)
                .await
                .map_err(map)?
            {
                Some(message) => Ok(ok(&json!({ "data": message }))),
                None => Err(HttpError::from(ApiError::MessageNotFoundError {
                    session_id: session_id.into_inner(),
                    message_id: raw.clone(),
                    message: format!("Message not found: {raw}"),
                })),
            }
        }
        "session.messages" => messages(&ctx, session).await,
        other => Err(HttpError::not_yet_ported(other)),
    }
}

async fn list(ctx: &Ctx, session: Arc<dyn SessionService>) -> Result<Response, HttpError> {
    let invalid = || {
        HttpError::from(ApiError::InvalidCursorError {
            message: "Invalid cursor".to_string(),
        })
    };
    let base = match ctx.query_str("cursor") {
        Some(raw) => SessionsCursor::parse(&raw).ok_or_else(invalid)?,
        None => SessionsCursor {
            workspace: ctx.query_str("workspace"),
            order: ctx.query_str("order"),
            search: ctx.query_str("search"),
            directory: ctx.query_str("directory"),
            project: ctx.query_str("project"),
            subpath: ctx.query_str("subpath"),
            anchor: ListAnchor {
                id: SessionID::create(),
                time: Finite::from(0),
                direction: Direction::Next,
            },
        },
    };
    let from_cursor = ctx.query.contains_key("cursor");
    // `"directory" in input` wins over `"project" in input`, mirroring the union decode order.
    let scope = match (&base.directory, &base.project) {
        (Some(directory), _) => ListScope::Directory(directory.clone()),
        (None, Some(project)) => ListScope::Project {
            project: ProjectID::new(project.clone()),
            subpath: base.subpath.clone().map(RelativePath::new),
        },
        _ => ListScope::All,
    };
    let request = ListRequest {
        scope,
        workspace_id: base
            .workspace
            .clone()
            .map(|w| WorkspaceID::new(w).map_err(|e| HttpError::Defect(e.to_string())))
            .transpose()?,
        search: base.search.clone(),
        limit: Some(
            ctx.query_num("limit")
                .map(|n| n as u32)
                .unwrap_or(DEFAULT_SESSIONS_LIMIT),
        ),
        order: base.order.as_deref().map(order),
        anchor: from_cursor.then(|| base.anchor.clone()),
    };
    let sessions = session.list(request).await.map_err(map)?;
    let page_cursor = |info: &opencode_session_api::info::SessionInfo, direction: Direction| {
        SessionsCursor {
            anchor: ListAnchor {
                id: info.id.clone(),
                time: Finite::new(info.time.created.millis() as f64).unwrap_or(Finite::from(0)),
                direction,
            },
            ..base.clone()
        }
        .encode()
    };
    let mut cursor = Map::new();
    if let Some(first) = sessions.first() {
        cursor.insert(
            "previous".into(),
            Value::String(page_cursor(first, Direction::Previous)),
        );
    }
    if let Some(last) = sessions.last() {
        cursor.insert(
            "next".into(),
            Value::String(page_cursor(last, Direction::Next)),
        );
    }
    Ok(ok(&json!({ "data": sessions, "cursor": cursor })))
}

async fn create(ctx: &Ctx, session: Arc<dyn SessionService>) -> Result<Response, HttpError> {
    let payload = ctx.payload.clone().unwrap_or_else(|| json!({}));
    let field = |name: &str| payload.get(name).cloned();
    let request = CreateRequest {
        id: field("id").map(|v| typed(true, "Payload", v)).transpose()?,
        agent: field("agent")
            .map(|v| typed(true, "Payload", v))
            .transpose()?,
        model: field("model")
            .map(|v| typed(true, "Payload", v))
            .transpose()?,
        location: match field("location") {
            Some(v) => typed(true, "Payload", v)?,
            None => opencode_session_api::ids::LocationRef {
                directory: AbsolutePath::new(current_dir()),
                workspace_id: None,
            },
        },
    };
    let info = session.create(request).await.map_err(map)?;
    Ok(ok(&json!({ "data": info })))
}

async fn prompt(ctx: &Ctx, session: Arc<dyn SessionService>) -> Result<Response, HttpError> {
    let request = PromptRequest {
        id: optional(ctx, "id")?,
        session_id: sid(ctx)?,
        prompt: typed(true, "Payload", payload_field(ctx, "prompt"))?,
        delivery: optional(ctx, "delivery")?,
        resume: payload_field(ctx, "resume").as_bool(),
    };
    let admitted = session.prompt(request).await.map_err(map)?;
    Ok(ok(&json!({ "data": admitted })))
}

async fn messages(ctx: &Ctx, session: Arc<dyn SessionService>) -> Result<Response, HttpError> {
    let raw_cursor = ctx.query_str("cursor");
    let requested_order = ctx.query_str("order");
    if raw_cursor.is_some() && requested_order.is_some() {
        return Err(HttpError::from(ApiError::InvalidCursorError {
            message: "Cursor cannot be combined with order".to_string(),
        }));
    }
    let decoded = match raw_cursor {
        Some(raw) => Some(MessageCursor::decode(&raw).ok_or_else(|| {
            HttpError::from(ApiError::InvalidCursorError {
                message: "Invalid cursor".to_string(),
            })
        })?),
        None => None,
    };
    let order_name = decoded
        .as_ref()
        .map(|c| c.order.clone())
        .or(requested_order)
        .unwrap_or_else(|| "desc".to_string());
    let cursor = match &decoded {
        Some(c) => Some((
            typed::<MessageID>(true, "Query", Value::String(c.id.clone()))?,
            if c.direction == "previous" {
                CursorDirection::Previous
            } else {
                CursorDirection::Next
            },
        )),
        None => None,
    };
    let request = MessagesRequest {
        session_id: sid(ctx)?,
        limit: Some(
            ctx.query_num("limit")
                .map(|n| n as u32)
                .unwrap_or(DEFAULT_MESSAGES_LIMIT),
        ),
        order: Some(order(&order_name)),
        cursor,
    };
    let list = session.messages(request).await.map_err(map)?;
    let id_of = |m: &opencode_session_api::message::Message| {
        serde_json::to_value(m)
            .ok()
            .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default()
    };
    let mut cursor = Map::new();
    if let Some(first) = list.first() {
        cursor.insert(
            "previous".into(),
            Value::String(MessageCursor::encode(
                &id_of(first),
                &order_name,
                "previous",
            )),
        );
    }
    if let Some(last) = list.last() {
        cursor.insert(
            "next".into(),
            Value::String(MessageCursor::encode(&id_of(last), &order_name, "next")),
        );
    }
    Ok(ok(&json!({ "data": list, "cursor": cursor })))
}

fn order(value: &str) -> Order {
    if value == "asc" {
        Order::Asc
    } else {
        Order::Desc
    }
}

fn sid(ctx: &Ctx) -> Result<SessionID, HttpError> {
    typed(true, "Params", Value::String(ctx.param("sessionID")))
}

fn payload_field(ctx: &Ctx, name: &str) -> Value {
    ctx.payload
        .as_ref()
        .and_then(|p| p.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

fn payload_str(ctx: &Ctx, name: &str) -> String {
    payload_field(ctx, name)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn optional<T: serde::de::DeserializeOwned>(ctx: &Ctx, name: &str) -> Result<Option<T>, HttpError> {
    match payload_field(ctx, name) {
        Value::Null => Ok(None),
        value => typed(true, "Payload", value).map(Some),
    }
}

/// Domain error -> HTTP envelope (`packages/server/src/handlers/session.ts` catchTag chains).
pub fn map(error: SessionError) -> HttpError {
    match error {
        SessionError::NotFound { session_id } => HttpError::from(ApiError::SessionNotFoundError {
            message: format!("Session not found: {}", session_id.as_str()),
            session_id: session_id.into_inner(),
        }),
        SessionError::MessageNotFound {
            session_id,
            message_id,
        } => HttpError::from(ApiError::MessageNotFoundError {
            message: format!("Message not found: {}", message_id.as_str()),
            session_id: session_id.into_inner(),
            message_id: message_id.into_inner(),
        }),
        SessionError::PromptConflict { message_id, .. } => {
            HttpError::from(ApiError::ConflictError {
                message: format!(
                    "Prompt message ID conflicts with an existing durable record: {}",
                    message_id.as_str()
                ),
                resource: Some(message_id.into_inner()),
            })
        }
        SessionError::OperationUnavailable { operation } => {
            HttpError::from(ApiError::ServiceUnavailableError {
                message: format!("Session {} is not available yet", operation.as_str()),
                service: Some(format!("session.{}", operation.as_str())),
            })
        }
        // Storage failures are `orDie` in the baseline: they take the defect path below.
        error @ (SessionError::MessageDecode { .. }
        | SessionError::ContextSnapshotDecode { .. }) => {
            let reference = defect_ref();
            tracing::error!(reference, %error, tag = error.tag(), "session operation failed");
            HttpError::from(ApiError::UnknownError {
                message: DEFECT_MESSAGE.to_string(),
                reference: Some(reference),
            })
        }
        other => HttpError::Defect(format!("{}: {other}", other.tag())),
    }
}
