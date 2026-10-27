//! Purpose: the real `ChildHost` composition for the task tool: legacy Session reads, child Session
//! creation through the legacy `session.created` event and calling-message model reads over the
//! existing SQLite tables, plus the `ChildOps` port for the seams owned by other nodes.
//! Scope: Location-scoped adapter over `SessionEvents` (database + event store).
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/lib.rs
//! |-- session-runtime/src/children/types.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/service.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/children.rs
//! Invariants: child Sessions are written only by the legacy `session.created` projector (no direct
//! row writes). `get` fails with `Session not found: <id>` like legacy `Session.get`. Seams not wired
//! yet fail with `ChildError::NotYetPorted` through `UnwiredChildOps`, never a silent success.
//! Contract tests: session-runtime/tests/children.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::{Definition, LocationRef, PublishOptions};
use opencode_schema::SessionID;
use opencode_storage_api::{DatabaseExt, Row, Sql, StorageError};
use serde_json::{Map, Value, json};

use super::types::{
    AgentInfo, CallingModel, ChildError, ChildHost, ChildReply, ModelSel, PermissionRule,
    SessionNode, TaskInput, TaskRun,
};
use crate::projection::{SessionEvents, now_millis};
use crate::service::slug;

/// `InstanceState.context` values legacy `Session.create` stamps on a child.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildLocation {
    pub project_id: String,
    pub directory: String,
    /// `sessionPath(worktree, directory)`.
    pub path: Option<String>,
    pub workspace_id: Option<String>,
    /// `InstallationVersion`.
    pub version: String,
}

/// Legacy `Session.Service` reads/creates and `MessageV2.get` used by the task tool.
#[derive(Clone)]
pub struct LegacySessions {
    events: Arc<dyn SessionEvents>,
    location: ChildLocation,
}

impl LegacySessions {
    pub fn new(events: Arc<dyn SessionEvents>, location: ChildLocation) -> Self {
        Self { events, location }
    }

    /// Legacy `Session.get`.
    pub async fn get(&self, id: &str) -> Result<SessionNode, ChildError> {
        let key = id.to_owned();
        let row = self
            .events
            .database()
            .read(move |tx: &dyn Sql| {
                tx.query_row(
                    "SELECT id, parent_id, agent, permission FROM session WHERE id = ?",
                    &[key.into()],
                )
            })
            .await
            .map_err(storage)?;
        let row = row.ok_or_else(|| ChildError::NotFound(format!("Session not found: {id}")))?;
        node(&row).map_err(storage)
    }

    /// Legacy `Session.create({ parentID, title, agent, permission })`.
    pub async fn create_child(
        &self,
        parent_id: &str,
        title: String,
        agent: &str,
        permission: Vec<PermissionRule>,
    ) -> Result<SessionNode, ChildError> {
        let id = SessionID::descending(None);
        let now = now_millis();
        let mut info = json!({
            "id": id,
            "slug": slug(),
            "version": self.location.version,
            "projectID": self.location.project_id,
            "directory": self.location.directory,
        });
        let object = info.as_object_mut().expect("object literal");
        if let Some(path) = &self.location.path {
            object.insert("path".into(), path.as_str().into());
        }
        if let Some(workspace) = &self.location.workspace_id {
            object.insert("workspaceID".into(), workspace.as_str().into());
        }
        object.insert("parentID".into(), parent_id.into());
        object.insert("title".into(), title.into());
        object.insert("agent".into(), agent.into());
        object.insert(
            "permission".into(),
            serde_json::to_value(&permission).unwrap_or(Value::Null),
        );
        object.insert("cost".into(), 0.into());
        object.insert(
            "tokens".into(),
            json!({"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}),
        );
        object.insert("time".into(), json!({"created": now, "updated": now}));
        self.events
            .store()
            .publish(
                &Definition::durable("session.created", 1, "sessionID"),
                json!({"sessionID": id, "info": info}),
                PublishOptions {
                    location: Some(LocationRef {
                        directory: self.location.directory.clone(),
                        workspace_id: self.location.workspace_id.clone(),
                    }),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(|error| ChildError::Storage {
                retryable: error.is_retryable(),
                message: error.to_string(),
            })?;
        Ok(SessionNode {
            id: id.to_string(),
            parent_id: Some(parent_id.to_owned()),
            agent: Some(agent.to_owned()),
            permission,
        })
    }

    /// `MessageV2.get({ sessionID, messageID })` narrowed to the assistant model fields.
    pub async fn calling_model(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<CallingModel, ChildError> {
        let (session, message) = (session_id.to_owned(), message_id.to_owned());
        let row = self
            .events
            .database()
            .read(move |tx: &dyn Sql| {
                tx.query_row(
                    "SELECT data FROM message WHERE id = ? AND session_id = ?",
                    &[message.into(), session.into()],
                )
            })
            .await
            .map_err(storage)?
            .ok_or_else(|| ChildError::NotFound(format!("Message not found: {message_id}")))?;
        let data: Map<String, Value> = row.get_json("data").map_err(storage)?;
        if data.get("role").and_then(Value::as_str) != Some("assistant") {
            return Err(ChildError::Failed("Not an assistant message".into()));
        }
        let text = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(CallingModel {
            model: ModelSel {
                model_id: text("modelID").unwrap_or_default(),
                provider_id: text("providerID").unwrap_or_default(),
            },
            variant: text("variant"),
        })
    }
}

/// Seams of the task tool owned outside this node (Phase 91 composition wires the producers):
/// AgentV2 lookup, the `task` permission ask, tool-context metadata and the legacy SessionPrompt ops.
#[async_trait]
pub trait ChildOps: Send + Sync {
    fn scope(&self) -> Option<Arc<dyn Send + Sync>> {
        None
    }
    /// `Agent.get(name)`.
    async fn agent(&self, name: &str) -> Result<Option<AgentInfo>, ChildError>;
    /// `ctx.ask({ permission, patterns: [pattern], always: ["*"], metadata })`.
    async fn ask_permission(
        &self,
        input: &TaskInput,
        permission: &str,
        pattern: &str,
    ) -> Result<(), ChildError>;
    /// `ctx.metadata({ title, metadata })`.
    async fn tool_metadata(&self, input: &TaskInput, title: &str, metadata: &Map<String, Value>);
    /// `ctx.extra.promptOps` presence ("TaskTool requires promptOps in ctx.extra").
    fn prompt_ready(&self) -> Result<(), ChildError>;
    /// `resolvePromptParts` then `ops.prompt` on the child.
    async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError>;
    /// `ops.cancel(sessionID)`.
    async fn cancel(&self, session_id: &str);
    /// `ops.prompt` with one synthetic text part on `session_id` using `agent`.
    async fn prompt_parent(
        &self,
        session_id: &str,
        agent: &str,
        variant: Option<String>,
        text: String,
    ) -> Result<(), ChildError>;
}

/// `ChildOps` before composition: every capability fails with a typed `NotYetPorted`.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnwiredChildOps;

pub const UNWIRED_AGENT: &str = "task tool agent lookup (AgentV2 registry composition, phase 91)";
pub const UNWIRED_PERMISSION: &str = "task tool permission ask (opencode-permissions, phase 05)";
pub const UNWIRED_PROMPT: &str = "task tool legacy SessionPrompt ops (p04.legacy)";

#[async_trait]
impl ChildOps for UnwiredChildOps {
    async fn agent(&self, _: &str) -> Result<Option<AgentInfo>, ChildError> {
        Err(ChildError::NotYetPorted(UNWIRED_AGENT))
    }
    async fn ask_permission(&self, _: &TaskInput, _: &str, _: &str) -> Result<(), ChildError> {
        Err(ChildError::NotYetPorted(UNWIRED_PERMISSION))
    }
    async fn tool_metadata(&self, _: &TaskInput, _: &str, _: &Map<String, Value>) {}
    fn prompt_ready(&self) -> Result<(), ChildError> {
        Err(ChildError::NotYetPorted(UNWIRED_PROMPT))
    }
    async fn prompt(&self, _: &TaskRun) -> Result<ChildReply, ChildError> {
        Err(ChildError::NotYetPorted(UNWIRED_PROMPT))
    }
    async fn cancel(&self, _: &str) {}
    async fn prompt_parent(
        &self,
        _: &str,
        _: &str,
        _: Option<String>,
        _: String,
    ) -> Result<(), ChildError> {
        Err(ChildError::NotYetPorted(UNWIRED_PROMPT))
    }
}

/// `ChildHost` over the SQLite legacy Session tables plus the `ChildOps` seams.
#[derive(Clone)]
pub struct ComposedChildHost {
    sessions: LegacySessions,
    ops: Arc<dyn ChildOps>,
}

impl ComposedChildHost {
    pub fn new(sessions: LegacySessions, ops: Arc<dyn ChildOps>) -> Self {
        Self { sessions, ops }
    }
}

#[async_trait]
impl ChildHost for ComposedChildHost {
    fn scope(&self) -> Option<Arc<dyn Send + Sync>> {
        self.ops.scope()
    }
    async fn session(&self, id: &str) -> Result<SessionNode, ChildError> {
        self.sessions.get(id).await
    }
    async fn agent(&self, name: &str) -> Result<Option<AgentInfo>, ChildError> {
        self.ops.agent(name).await
    }
    async fn ask_permission(
        &self,
        input: &TaskInput,
        permission: &str,
        pattern: &str,
    ) -> Result<(), ChildError> {
        self.ops.ask_permission(input, permission, pattern).await
    }
    async fn create_child(
        &self,
        parent_id: &str,
        title: String,
        agent: &str,
        permission: Vec<PermissionRule>,
    ) -> Result<SessionNode, ChildError> {
        self.sessions
            .create_child(parent_id, title, agent, permission)
            .await
    }
    async fn calling_model(&self, input: &TaskInput) -> Result<CallingModel, ChildError> {
        self.sessions
            .calling_model(&input.session_id, &input.message_id)
            .await
    }
    async fn tool_metadata(&self, input: &TaskInput, title: &str, metadata: &Map<String, Value>) {
        self.ops.tool_metadata(input, title, metadata).await
    }
    fn prompt_ready(&self) -> Result<(), ChildError> {
        self.ops.prompt_ready()
    }
    async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError> {
        self.ops.prompt(run).await
    }
    async fn cancel(&self, session_id: &str) {
        self.ops.cancel(session_id).await
    }
    async fn inject_parent(
        &self,
        input: &TaskInput,
        variant: Option<String>,
        text: String,
    ) -> Result<(), ChildError> {
        let parent = self.sessions.get(&input.session_id).await?;
        let agent = parent.agent.unwrap_or_else(|| input.agent.clone());
        self.ops
            .prompt_parent(&input.session_id, &agent, variant, text)
            .await
    }
}

fn node(row: &Row) -> Result<SessionNode, StorageError> {
    let text = |column: &str| -> Result<Option<String>, StorageError> {
        Ok(row
            .get::<Option<String>>(column)?
            .filter(|value| !value.is_empty()))
    };
    Ok(SessionNode {
        id: row.get("id")?,
        parent_id: text("parent_id")?,
        agent: text("agent")?,
        permission: row
            .get_json_opt::<Vec<PermissionRule>>("permission")?
            .unwrap_or_default(),
    })
}

fn storage(error: StorageError) -> ChildError {
    ChildError::Storage {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}
