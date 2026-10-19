//! Purpose: adapt legacy and canonical permission/PTY HTTP operations to the shared native Location services.
//! Scope: JSON boundary conversion, no independent authorization policy or terminal registry.
//! Uses:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! |-- platform-api/src/pty.rs
//! |-- runtime/src/location.rs
//! |-- server/src/middleware/legacy_routing.rs
//! `-- server/src/services/legacy.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/native_services.rs
//! Invariants: HTTP replies settle the very permission assertion blocking the tool runner.
//! Contract tests: runtime/tests/permissions.rs

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError, PermissionRequest, ReplyInput, Source};
use opencode_platform_api::pty::{PtyError, PtyService, PtyStatus};
use opencode_schema::location::Ref;
use opencode_schema::values::AbsolutePath;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyError, LegacySessions};
use serde_json::{Map, Value, json};

macro_rules! encode {
    ($value:expr) => {
        serde_json::to_value($value).map_err(|error| LegacyError::Internal(error.to_string()))
    };
}

pub struct LegacyNative {
    pub locations: crate::location::Locations,
}

impl LegacyNative {
    pub fn new(locations: crate::location::Locations) -> Self {
        Self { locations }
    }
}

#[async_trait]
impl LegacySessions for LegacyNative {
    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let lease = self
            .locations
            .get(&Ref {
                directory: AbsolutePath::new(context.directory.clone()),
                workspace_id: context
                    .workspace_id
                    .clone()
                    .map(opencode_schema::WorkspaceID::new)
                    .transpose()
                    .map_err(bad)?,
            })
            .await
            .map_err(|error| LegacyError::Internal(error.to_string()))?;
        let permission = &lease.permissions.service;
        let pty = &lease.native.pty;
        let body = payload.unwrap_or_else(|| json!({}));
        match operation {
            "tool.ids" => encode!(lease.tools.materialize(&[]).names()),
            "tool.list" => Ok(Value::Array(lease.tools.materialize(&[]).definitions().into_iter().map(|tool| json!({"id":tool.name,"description":tool.description,"parameters":tool.input_schema})).collect())),
            "permission.list" => Ok(Value::Array(permission.list().into_iter().map(legacy_request).collect())),
            "v2.permission.request.list" => encode!(permission.list()),
            "v2.session.permission.list" => encode!(permission.for_session(required(params, "sessionID")?)),
            "v2.session.permission.get" => {
                let id = required(params, "requestID")?;
                let request = permission.get(id).filter(|request| request.session_id == params["sessionID"]);
                encode!(request.ok_or_else(|| missing_permission(id))?)
            }
            "permission.reply" | "permission.respond" | "v2.session.permission.reply" => {
                let id = required(params, if operation == "permission.respond" { "permissionID" } else { "requestID" })?;
                if operation.starts_with("v2.") && permission.get(id).is_none_or(|request| request.session_id != params["sessionID"]) {
                    return Err(missing_permission(id));
                }
                permission.reply(ReplyInput { request_id: id.to_owned(), reply: serde_json::from_value(body[if operation == "permission.respond" { "response" } else { "reply" }].clone()).map_err(bad)?, message: body.get("message").and_then(Value::as_str).map(str::to_owned) }).await.map_err(permission_error)?;
                Ok(json!(true))
            }
            "v2.session.permission.create" => {
                let mut input = AssertInput::new(required(params, "sessionID")?, text(&body, "action")?, serde_json::from_value(body["resources"].clone()).map_err(bad)?);
                input.id = body.get("id").and_then(Value::as_str).map(str::to_owned);
                input.agent = body.get("agent").and_then(Value::as_str).map(str::to_owned);
                input.save = body.get("save").cloned().map(serde_json::from_value).transpose().map_err(bad)?;
                input.metadata = body.get("metadata").and_then(Value::as_object).cloned();
                input.source = body.get("source").cloned().map(serde_json::from_value).transpose().map_err(bad)?;
                encode!(permission.ask(input).await.map_err(permission_error)?)
            }
            "v2.permission.saved.list" => encode!(lease.permissions.saved.list(Some(query.get("projectID").and_then(Value::as_str).unwrap_or(lease.info.project.id.as_str()))).await.map_err(permission_error)?),
            "v2.permission.saved.remove" => {
                lease.permissions.saved.remove(required(params, "id")?).await.map_err(permission_error)?;
                Ok(Value::Null)
            }
            "question.list" | "v2.question.request.list" => encode!(lease.questions.list()),
            "v2.session.question.list" => encode!(lease.questions.list().into_iter().filter(|request| request.session_id.as_str() == params["sessionID"].as_str().unwrap_or_default()).collect::<Vec<_>>()),
            "question.reply" | "question.reject" | "v2.session.question.reply" | "v2.session.question.reject" => {
                let id = required(params, "requestID")?;
                if operation.starts_with("v2.") && !lease.questions.list().iter().any(|request| request.id.as_str() == id && request.session_id.as_str() == params["sessionID"].as_str().unwrap_or_default()) {
                    return Err(question_error(opencode_tools::interaction::QuestionError::NotFound(id.into())));
                }
                if operation.ends_with(".reply") {
                    lease.questions.reply(id, serde_json::from_value(body["answers"].clone()).map_err(bad)?).await.map_err(question_error)?;
                } else {
                    lease.questions.reject(id).await.map_err(question_error)?;
                }
                Ok(json!(true))
            }
            "pty.list" => encode!(pty.list().await.into_iter().filter(|info| info.status == PtyStatus::Running).collect::<Vec<_>>()),
            "v2.pty.list" => encode!(pty.list().await),
            "pty.create" | "v2.pty.create" => encode!(pty.create(serde_json::from_value(body).map_err(bad)?).await.map_err(pty_error)?),
            "pty.get" | "v2.pty.get" => {
                let id = required(params, "ptyID")?;
                let info = pty.get(id).await.map_err(pty_error)?;
                if operation == "pty.get" && info.status != PtyStatus::Running { return Err(pty_error(PtyError::NotFound(id.to_owned()))); }
                encode!(info)
            }
            "pty.update" | "v2.pty.update" => {
                let id = required(params, "ptyID")?;
                if operation == "pty.update" && pty.get(id).await.map_err(pty_error)?.status != PtyStatus::Running { return Err(pty_error(PtyError::NotFound(id.to_owned()))); }
                encode!(pty.update(id, serde_json::from_value(body).map_err(bad)?).await.map_err(pty_error)?)
            }
            "pty.remove" | "v2.pty.remove" => {
                let id = required(params, "ptyID")?;
                if operation == "pty.remove" && pty.get(id).await.map_err(pty_error)?.status != PtyStatus::Running { return Err(pty_error(PtyError::NotFound(id.to_owned()))); }
                pty.remove(id).await.map_err(pty_error)?;
                Ok(json!(true))
            }
            "pty.shells" | "v2.pty.shells" => super::shells().await,
            _ => Err(LegacyError::Unavailable(format!("Native operation is not implemented: {operation}"))),
        }
    }
}

fn legacy_request(request: PermissionRequest) -> Value {
    let mut output = json!({"id": request.id, "sessionID": request.session_id, "permission": request.action, "patterns": request.resources, "always": request.save.unwrap_or_default(), "metadata": request.metadata.unwrap_or_default()});
    if let Some(Source::Tool {
        message_id,
        call_id,
    }) = request.source
    {
        output["tool"] = json!({"messageID":message_id,"callID":call_id});
    }
    output
}

fn required<'a>(params: &'a Map<String, Value>, key: &str) -> Result<&'a str, LegacyError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| LegacyError::BadRequest(format!("Missing {key}")))
}
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, LegacyError> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| LegacyError::BadRequest(format!("Missing {key}")))
}
fn bad(error: impl std::fmt::Display) -> LegacyError {
    LegacyError::BadRequest(error.to_string())
}
fn missing_permission(id: &str) -> LegacyError {
    LegacyError::Response {
        status: 404,
        body: json!({"_tag":"PermissionNotFoundError", "requestID":id, "message":format!("Permission request not found: {id}")}),
    }
}
fn permission_error(error: PermissionError) -> LegacyError {
    match error {
        PermissionError::NotFound { request_id } => missing_permission(&request_id),
        PermissionError::SessionNotFound { session_id } => LegacyError::Response {
            status: 404,
            body: json!({"_tag":"SessionNotFoundError","sessionID":session_id,"message":format!("Session not found: {session_id}")}),
        },
        error => LegacyError::Internal(error.to_string()),
    }
}
fn question_error(error: opencode_tools::interaction::QuestionError) -> LegacyError {
    match error {
        opencode_tools::interaction::QuestionError::NotFound(id) => LegacyError::Response {
            status: 404,
            body: json!({"_tag":"QuestionNotFoundError","requestID":id,"message":format!("Question request not found: {id}")}),
        },
        error => LegacyError::Internal(error.to_string()),
    }
}

fn pty_error(error: PtyError) -> LegacyError {
    match error {
        PtyError::NotFound(id) | PtyError::Exited(id) => LegacyError::Response {
            status: 404,
            body: json!({"_tag":"PtyNotFoundError","ptyID":id,"message":format!("PTY session not found: {id}")}),
        },
        error => LegacyError::Internal(error.to_string()),
    }
}
