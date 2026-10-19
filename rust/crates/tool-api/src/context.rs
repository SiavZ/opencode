//! Purpose: per-invocation tool context and the ports a leaf tool uses: authorization through the
//! single permission service, progress/metadata updates, and cancellation.
//! Scope: values and traits.
//! Uses:
//! rust/crates/
//! `-- permissions/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/lib.rs
//! `-- tool-api/src/registry.rs
//! Invariants: leaves authorize before reading target content or causing side effects.
//! Contract tests: tool-api/tests/registry.rs

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Authorization port. Production implementation is `permissions::PermissionService::assert`.
#[async_trait]
pub trait Authorizer: Send + Sync {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError>;
}

#[async_trait]
impl Authorizer for opencode_permissions::PermissionService {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        opencode_permissions::PermissionService::assert(self, input).await
    }
}

/// Live progress for running tools (legacy `ctx.metadata({title, metadata})`).
pub trait ToolProgress: Send + Sync {
    fn update(&self, title: Option<&str>, metadata: &Map<String, Value>);
}

#[derive(Debug, Default)]
pub struct NoProgress;

impl ToolProgress for NoProgress {
    fn update(&self, _title: Option<&str>, _metadata: &Map<String, Value>) {}
}

/// `Tool.Context` plus the explicit ports.
#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub agent: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub cancel: CancellationToken,
    pub authorizer: Arc<dyn Authorizer>,
    pub progress: Arc<dyn ToolProgress>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("session_id", &self.session_id)
            .field("agent", &self.agent)
            .field("assistant_message_id", &self.assistant_message_id)
            .field("tool_call_id", &self.tool_call_id)
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    /// Assert a permission with this invocation's session, agent and tool source filled in.
    pub async fn authorize(
        &self,
        action: &str,
        resources: Vec<String>,
        save: Option<Vec<String>>,
        metadata: Option<Map<String, Value>>,
    ) -> Result<(), PermissionError> {
        let mut input = AssertInput::new(self.session_id.clone(), action, resources);
        input.save = save;
        input.metadata = metadata;
        input.agent = Some(self.agent.clone());
        input.source = Some(opencode_permissions::Source::Tool {
            message_id: self.assistant_message_id.clone(),
            call_id: self.tool_call_id.clone(),
        });
        self.authorizer.assert(input).await
    }
}
