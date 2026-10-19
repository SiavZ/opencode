//! Purpose: service traits for the public Session operations and process-global execution,
//! mirroring `SessionV2.Interface` and `SessionExecution.Interface`. Implemented in
//! session-runtime, consumed by server and tools.
//! Scope: object-safe async traits.
//! Uses:
//! rust/crates/
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/src/info.rs
//! `-- session-api/src/message.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/commands/terminal.rs
//! `-- opencode/src/cli/commands/turn.rs
//! rust/crates/
//! |-- runtime/tests/process.rs
//! |-- runtime/tests/production.rs
//! |-- server/src/handlers/session.rs
//! |-- server/src/services/mod.rs
//! |-- server/tests/support/mod.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! |-- session-runtime/tests/admission_projection.rs
//! |-- session-runtime/tests/coordinator_execution.rs
//! |-- session-runtime/tests/runner_turn.rs
//! |-- session-runtime/tests/runner_turn_tools.rs
//! |-- session-runtime/tests/support/provider.rs
//! `-- session-runtime/tests/support/sql.rs
//! Invariants: no trait method takes a Session-scoped service; execution is keyed by Session ID.
//! `prompt` commits admission before any wake; `interrupt` on idle is a no-op.
//! Contract tests: session-runtime/tests/admission.rs, session-runtime/tests/coordinator.rs

use std::collections::BTreeSet;

use async_trait::async_trait;

use crate::admission::{Admitted, PromptRequest};
use crate::error::SessionResult;
use crate::ids::{
    AgentID, LocationRef, MessageID, ModelRef, ProjectID, RelativePath, SessionID, WorkspaceID,
};
use crate::info::{ListAnchor, RevertState, SessionInfo};
use crate::message::Message;

/// `SessionV2.create` input.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateRequest {
    pub id: Option<SessionID>,
    pub agent: Option<AgentID>,
    pub model: Option<ModelRef>,
    pub location: LocationRef,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    Asc,
    #[default]
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorDirection {
    Previous,
    Next,
}

/// Scope of `SessionV2.list` (`ListDirectoryInput | ListProjectInput | ListAllInput`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ListScope {
    #[default]
    All,
    Directory(String),
    Project {
        project: ProjectID,
        subpath: Option<RelativePath>,
    },
}

/// `SessionV2.list` input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListRequest {
    pub scope: ListScope,
    pub workspace_id: Option<WorkspaceID>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub order: Option<Order>,
    pub anchor: Option<ListAnchor>,
}

/// `SessionV2.history` page of durable Session events (encoded payload JSON, seq ordered).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPage {
    pub events: Vec<serde_json::Value>,
    pub has_more: bool,
}

/// `SessionV2.messages` input.
#[derive(Clone, Debug, PartialEq)]
pub struct MessagesRequest {
    pub session_id: SessionID,
    pub limit: Option<u32>,
    pub order: Option<Order>,
    pub cursor: Option<(MessageID, CursorDirection)>,
}

/// `SessionV2.revert.stage` input.
#[derive(Clone, Debug, PartialEq)]
pub struct RevertStageRequest {
    pub session_id: SessionID,
    pub message_id: MessageID,
    pub files: Option<bool>,
}

/// Process-global execution keyed by Session ID (`SessionExecution.Interface`).
#[async_trait]
pub trait SessionExecution: Send + Sync {
    /// Snapshot of Sessions with execution owned by this process.
    async fn active(&self) -> BTreeSet<SessionID>;
    /// Starts execution while idle or joins the active execution (force=true drain).
    async fn resume(&self, session_id: &SessionID) -> SessionResult<()>;
    /// Registers newly recorded work. Repeated wakeups may coalesce (force=false drain).
    async fn wake(&self, session_id: &SessionID);
    /// Interrupts active work owned by this process and waits for its cleanup. Idle is a no-op.
    async fn interrupt(&self, session_id: &SessionID);
}

/// Public Session operations (`SessionV2.Interface`).
#[async_trait]
pub trait SessionService: Send + Sync {
    /// Host lifecycle extension over the shared Session row (not part of the TS V2 interface).
    async fn rename(&self, _session_id: &SessionID, _title: &str) -> SessionResult<()> {
        Err(crate::SessionError::Validation(
            "Host rename service is not composed".into(),
        ))
    }
    async fn delete(&self, _session_id: &SessionID) -> SessionResult<()> {
        Err(crate::SessionError::Validation(
            "Host delete service is not composed".into(),
        ))
    }
    async fn fork(
        &self,
        _session_id: &SessionID,
        _message_id: Option<&MessageID>,
    ) -> SessionResult<SessionInfo> {
        Err(crate::SessionError::Validation(
            "Host fork service is not composed".into(),
        ))
    }
    async fn list(&self, request: ListRequest) -> SessionResult<Vec<SessionInfo>>;
    async fn create(&self, request: CreateRequest) -> SessionResult<SessionInfo>;
    async fn get(&self, session_id: &SessionID) -> SessionResult<SessionInfo>;
    async fn messages(&self, request: MessagesRequest) -> SessionResult<Vec<Message>>;
    async fn message(
        &self,
        session_id: &SessionID,
        message_id: &MessageID,
    ) -> SessionResult<Option<Message>>;
    /// Runner-visible history after context-epoch and compaction selection.
    async fn context(&self, session_id: &SessionID) -> SessionResult<Vec<Message>>;
    /// `SessionV2.history`: durable Session events after `after`, at most `limit`.
    async fn history(
        &self,
        session_id: &SessionID,
        after: Option<i64>,
        limit: usize,
    ) -> SessionResult<HistoryPage>;
    /// Durable admission, then advisory wake unless `resume == Some(false)`.
    async fn prompt(&self, request: PromptRequest) -> SessionResult<Admitted>;
    async fn switch_agent(&self, session_id: &SessionID, agent: &str) -> SessionResult<()>;
    async fn switch_model(&self, session_id: &SessionID, model: ModelRef) -> SessionResult<()>;
    /// V2 reports `OperationUnavailable { Shell }`.
    async fn shell(&self, session_id: &SessionID, command: &str) -> SessionResult<()>;
    /// V2 reports `OperationUnavailable { Skill }`.
    async fn skill(&self, session_id: &SessionID, skill: &str) -> SessionResult<()>;
    /// V2 checks existence, then reports `OperationUnavailable { Compact }`.
    async fn compact(&self, session_id: &SessionID) -> SessionResult<()>;
    /// V2 checks existence, then reports `OperationUnavailable { Wait }`.
    async fn wait(&self, session_id: &SessionID) -> SessionResult<()>;
    async fn active(&self) -> BTreeSet<SessionID>;
    async fn resume(&self, session_id: &SessionID) -> SessionResult<()>;
    async fn interrupt(&self, session_id: &SessionID);
    async fn revert_stage(&self, request: RevertStageRequest) -> SessionResult<RevertState>;
    async fn revert_clear(&self, session_id: &SessionID) -> SessionResult<()>;
    async fn revert_commit(&self, session_id: &SessionID) -> SessionResult<()>;
}
