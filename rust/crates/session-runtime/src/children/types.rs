//! Purpose: values, port trait and pure helpers for the task-tool child Session lifecycle: tool input
//! and output, the `ChildHost` port, `deriveSubagentSessionPermission` and `renderOutput`.
//! Scope: pure values plus one object-safe async trait.
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/children.rs
//! `-- session-runtime/src/children/store.rs
//! Invariants: permission rules keep the legacy `PermissionV1.Rule` wire shape
//! (`permission`, `pattern`, `action`). `render_output` is byte-identical to task.ts `renderOutput`.
//! Contract tests: session-runtime/tests/children.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `PermissionV1.Rule`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    /// `allow | deny | ask`.
    pub action: String,
}

impl PermissionRule {
    pub fn deny(permission: &str) -> Self {
        Self {
            permission: permission.into(),
            pattern: "*".into(),
            action: "deny".into(),
        }
    }
}

/// `{ modelID, providerID }` as used by the legacy prompt input and agent config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSel {
    #[serde(rename = "modelID")]
    pub model_id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
}

/// Fields of `Agent.Info` the task tool reads.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInfo {
    pub name: String,
    pub permission: Vec<PermissionRule>,
    pub model: Option<ModelSel>,
}

/// Fields of a legacy `Session.Info` the task tool reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent: Option<String>,
    pub permission: Vec<PermissionRule>,
}

/// Task tool parameters plus the calling tool context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskInput {
    /// `ctx.sessionID` (the parent Session).
    pub session_id: String,
    /// `ctx.messageID` (the calling assistant message).
    pub message_id: String,
    /// `ctx.agent`.
    pub agent: String,
    pub description: String,
    pub prompt: String,
    pub subagent_type: String,
    pub task_id: Option<String>,
    pub command: Option<String>,
    pub background: Option<bool>,
    /// `ctx.extra.bypassAgentCheck`.
    pub bypass_agent_check: bool,
}

/// Model of the calling assistant message (`MessageV2.get(ctx.messageID)`).
#[derive(Clone, Debug, PartialEq)]
pub struct CallingModel {
    pub model: ModelSel,
    pub variant: Option<String>,
}

/// One child prompt run (`ops.prompt` input).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRun {
    pub child: String,
    pub model: ModelSel,
    pub variant: Option<String>,
    pub agent: String,
    pub prompt: String,
}

/// Outcome of a child prompt as observed in the returned message (`result.info`/`result.parts`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChildReply {
    /// Assistant error message (`error.data.message`, else `error.name`).
    pub error: Option<String>,
    /// Error of the last failed tool part.
    pub failed_tool: Option<String>,
    /// Text of the last text part.
    pub text: Option<String>,
}

/// Tool result.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskOutput {
    pub title: String,
    pub metadata: Map<String, Value>,
    pub output: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Completed,
    Error,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChildError {
    /// Tool failure surfaced to the model (`Effect.fail(new Error(..))`).
    #[error("{0}")]
    Failed(String),
    /// The parent turn aborted the foreground task.
    #[error("Task cancelled")]
    Cancelled,
    /// Permission ask was denied or rejected.
    #[error("{0}")]
    PermissionDenied(String),
    /// Legacy `NotFoundError` (`Session not found: <id>`, `Message not found: <id>`).
    #[error("{0}")]
    NotFound(String),
    /// Storage or event infrastructure failure.
    #[error("{message}")]
    Storage { message: String, retryable: bool },
    /// A capability the composition has not wired yet (typed blocker, never a silent success).
    #[error("not yet ported: {0}")]
    NotYetPorted(&'static str),
}

/// Legacy Session, agent, permission and prompt operations used by the task tool. Implemented by
/// the runtime composition over the legacy Session service, Agent service and SessionPrompt ops.
#[async_trait]
pub trait ChildHost: Send + Sync {
    /// Keep the concrete prompt owner alive only while a child job is active.
    fn scope(&self) -> Option<std::sync::Arc<dyn Send + Sync>> {
        None
    }
    async fn session(&self, id: &str) -> Result<SessionNode, ChildError>;
    async fn agent(&self, name: &str) -> Result<Option<AgentInfo>, ChildError>;
    /// `ctx.ask({ permission, patterns: [pattern], always: ["*"], metadata })`.
    async fn ask_permission(
        &self,
        input: &TaskInput,
        permission: &str,
        pattern: &str,
    ) -> Result<(), ChildError>;
    /// `sessions.create({ parentID, title, agent, permission })`.
    async fn create_child(
        &self,
        parent_id: &str,
        title: String,
        agent: &str,
        permission: Vec<PermissionRule>,
    ) -> Result<SessionNode, ChildError>;
    /// Model of the calling message. Fails with "Not an assistant message" for user messages.
    async fn calling_model(&self, input: &TaskInput) -> Result<CallingModel, ChildError>;
    /// `ctx.metadata({ title, metadata })`.
    async fn tool_metadata(&self, input: &TaskInput, title: &str, metadata: &Map<String, Value>);
    /// Fails like "TaskTool requires promptOps in ctx.extra" when no prompt ops are wired.
    fn prompt_ready(&self) -> Result<(), ChildError>;
    /// `resolvePromptParts` then `ops.prompt` on the child; returns the observed reply.
    async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError>;
    /// `ops.cancel(sessionID)`.
    async fn cancel(&self, session_id: &str);
    /// Synthetic text prompt to the parent with its current agent (falls back to `input.agent`).
    async fn inject_parent(
        &self,
        input: &TaskInput,
        variant: Option<String>,
        text: String,
    ) -> Result<(), ChildError>;

    /// `runTask`: prompt the child and map the reply to output text or a failure.
    async fn prompt_child(&self, run: TaskRun) -> Result<String, ChildError> {
        let reply = self.prompt(&run).await?;
        if let Some(message) = reply.error {
            return Err(ChildError::Failed(format!(
                "Subagent failed (task_id: {}): {message}",
                run.child
            )));
        }
        if let Some(message) = reply.failed_tool {
            return Err(ChildError::Failed(format!(
                "Subagent failed (task_id: {}): {message}",
                run.child
            )));
        }
        Ok(reply.text.unwrap_or_default())
    }
}

/// `deriveSubagentSessionPermission`: parent deny and external_directory rules, then default
/// `todowrite`/`task` denies unless the subagent's own rules mention them.
pub fn derive_subagent_permission(
    parent: &[PermissionRule],
    subagent: &AgentInfo,
) -> Vec<PermissionRule> {
    let has = |permission: &str| {
        subagent
            .permission
            .iter()
            .any(|rule| rule.permission == permission)
    };
    parent
        .iter()
        .filter(|rule| rule.permission == "external_directory" || rule.action == "deny")
        .cloned()
        .chain((!has("todowrite")).then(|| PermissionRule::deny("todowrite")))
        .chain((!has("task")).then(|| PermissionRule::deny("task")))
        .collect()
}

/// task.ts `renderOutput`.
pub fn render_output(
    session_id: &str,
    state: TaskState,
    summary: Option<&str>,
    text: &str,
) -> String {
    let (name, tag) = match state {
        TaskState::Running => ("running", "task_result"),
        TaskState::Completed => ("completed", "task_result"),
        TaskState::Error => ("error", "task_error"),
    };
    let mut lines = vec![format!("<task id=\"{session_id}\" state=\"{name}\">")];
    if let Some(summary) = summary.filter(|summary| !summary.is_empty()) {
        lines.push(format!("<summary>{summary}</summary>"));
    }
    lines.push(format!("<{tag}>"));
    lines.push(text.to_owned());
    lines.push(format!("</{tag}>"));
    lines.push("</task>".into());
    lines.join("\n")
}
