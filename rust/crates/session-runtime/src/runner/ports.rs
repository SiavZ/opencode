//! Purpose: runner ports for llm.ts collaborators: the `LlmClient` re-export, `ToolMaterializer`
//! (registry.ts materialize/settle) with its `ToolRegistry` adapter, the system-context loader,
//! automatic/overflow compaction and snapshots. Does not run provider turns.
//! Scope: traits plus the production adapter over opencode-tool-api and no-op ports for
//! capabilities whose owners have not landed (compaction, snapshots, guidance).
//! Uses:
//! rust/crates/
//! |-- context/src/registry.rs
//! |-- context/src/source/mod.rs
//! |-- llm-api/src/lib.rs
//! |-- permissions/src/lib.rs
//! |-- schema/src/permission.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/agents.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde::Serialize
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::to_value
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_tools.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/runner/turn/tools.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: a materialization is an immutable advertisement; settle never re-materializes.
//! Tool failures are model-facing `Settlement`s; only output-store failures (`Failed`) and user
//! declines (`Declined`) escape as errors, matching registry.ts and llm.ts `isUserDeclined`.
//! Contract tests: session-runtime/tests/runner_turn.rs, session-runtime/tests/runner_turn_tools.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_context::registry::Registry;
use opencode_context::source::{ContextError, SystemContext};
use opencode_llm_api::{
    JsonObject, LlmRequest, Model, ToolDefinition, ToolOutput, ToolResultValue,
};
use opencode_schema::permission::{Effect as SchemaEffect, Rule as SchemaRule};
use opencode_schema::values::RelativePath;
use opencode_session_api::{Message, MessageID, SessionError, SessionID};
use opencode_tool_api::{Authorizer, ExecuteInput, NoProgress, ToolContext, ToolRegistry};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agents::Selection;

pub use opencode_llm_api::LlmClient;

/// One local tool call to settle (`ToolRegistry.settle` input).
#[derive(Debug, Clone)]
pub struct SettleInput {
    pub session_id: SessionID,
    pub agent: String,
    pub assistant_message_id: MessageID,
    pub call_id: String,
    pub name: String,
    pub input: Value,
}

/// `ToolRegistry.Settlement` in llm-api terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Settlement {
    pub result: ToolResultValue,
    pub output: Option<ToolOutput>,
    pub output_paths: Vec<String>,
}

/// Settlement failures that are not model-facing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SettleError {
    /// `PermissionV2.DeclinedError` / `QuestionV2.RejectedError`: halts the loop (V1 parity).
    #[error("{0}")]
    Declined(String),
    /// `ToolOutputStore.Error` or another failure: fails unsettled tools of the turn.
    #[error("{0}")]
    Failed(String),
}

/// `ToolRegistry.materialize(permissions)` result.
#[async_trait]
pub trait MaterializedTools: Send + Sync {
    fn definitions(&self) -> Vec<ToolDefinition>;
    /// Settles one call. `cancel` fires when the turn is interrupted.
    async fn settle(
        &self,
        input: SettleInput,
        cancel: CancellationToken,
    ) -> Result<Settlement, SettleError>;
}

/// `ToolRegistry.Service` as the runner sees it.
pub trait ToolMaterializer: Send + Sync {
    fn materialize(&self, permissions: &[SchemaRule]) -> Arc<dyn MaterializedTools>;
}

/// Production adapter over the opencode-tool-api registry.
pub struct RegistryMaterializer {
    pub registry: ToolRegistry,
    pub authorizer: Arc<dyn Authorizer>,
}

impl ToolMaterializer for RegistryMaterializer {
    fn materialize(&self, permissions: &[SchemaRule]) -> Arc<dyn MaterializedTools> {
        let rules: Vec<opencode_permissions::Rule> = permissions.iter().map(rule).collect();
        Arc::new(RegistryTools {
            materialization: self.registry.materialize(&rules),
            authorizer: self.authorizer.clone(),
        })
    }
}

struct RegistryTools {
    materialization: opencode_tool_api::Materialization,
    authorizer: Arc<dyn Authorizer>,
}

#[async_trait]
impl MaterializedTools for RegistryTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.materialization
            .definitions()
            .into_iter()
            .map(|definition| ToolDefinition {
                name: definition.name,
                description: definition.description,
                input_schema: object(definition.input_schema),
                output_schema: definition.output_schema.map(object),
                cache: None,
                metadata: None,
                native: None,
            })
            .collect()
    }

    async fn settle(
        &self,
        input: SettleInput,
        cancel: CancellationToken,
    ) -> Result<Settlement, SettleError> {
        let context = ToolContext {
            session_id: input.session_id.as_str().to_owned(),
            agent: input.agent,
            assistant_message_id: input.assistant_message_id.as_str().to_owned(),
            tool_call_id: input.call_id,
            cancel,
            authorizer: self.authorizer.clone(),
            progress: Arc::new(NoProgress),
        };
        let call = ExecuteInput {
            name: input.name,
            input: input.input,
        };
        let settlement = self
            .materialization
            .settle(call, &context)
            .await
            .map_err(SettleError::Failed)?;
        // tool-api and llm-api share the `{type, value}` result and output wire shapes.
        let result = convert(&settlement.result)?;
        let output = settlement.output.as_ref().map(convert).transpose()?;
        Ok(Settlement {
            result,
            output,
            output_paths: settlement.output_paths,
        })
    }
}

fn rule(rule: &SchemaRule) -> opencode_permissions::Rule {
    let effect = match rule.effect {
        SchemaEffect::Allow => opencode_permissions::Effect::Allow,
        SchemaEffect::Deny => opencode_permissions::Effect::Deny,
        SchemaEffect::Ask => opencode_permissions::Effect::Ask,
    };
    opencode_permissions::Rule::new(rule.action.clone(), rule.resource.clone(), effect)
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn convert<A: serde::Serialize, B: serde::de::DeserializeOwned>(
    value: &A,
) -> Result<B, SettleError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| SettleError::Failed(error.to_string()))
}

/// `SystemContextRegistry.load` + `SkillGuidance.load(agent)` + `ReferenceGuidance.load`, combined.
#[async_trait]
pub trait SystemContextLoader: Send + Sync {
    async fn load(&self, agent: &Selection) -> Result<SystemContext, ContextError>;
}

/// Loads only the registry. Skill and reference guidance contribute through registry entries
/// until their owners land dedicated sources.
pub struct RegistryContext(pub Registry);

#[async_trait]
impl SystemContextLoader for RegistryContext {
    async fn load(&self, _agent: &Selection) -> Result<SystemContext, ContextError> {
        self.0.load().await
    }
}

/// `SessionCompaction.Input`.
pub struct CompactionInput<'a> {
    pub session_id: &'a SessionID,
    /// Runner history entries `(seq, message)`.
    pub entries: &'a [(i64, Message)],
    pub model: &'a Model,
    pub request: &'a LlmRequest,
}

/// `SessionCompaction.make(...)`: both return true when a compaction was committed.
#[async_trait]
pub trait Compaction: Send + Sync {
    async fn compact_if_needed(&self, input: &CompactionInput<'_>) -> Result<bool, SessionError>;
    async fn compact_after_overflow(
        &self,
        input: &CompactionInput<'_>,
    ) -> Result<bool, SessionError>;
}

/// Compaction disabled (`auto: false` and no context limit): never compacts.
pub struct NoCompaction;

#[async_trait]
impl Compaction for NoCompaction {
    async fn compact_if_needed(&self, _: &CompactionInput<'_>) -> Result<bool, SessionError> {
        Ok(false)
    }
    async fn compact_after_overflow(&self, _: &CompactionInput<'_>) -> Result<bool, SessionError> {
        Ok(false)
    }
}

/// `Snapshot.Service` subset used by the runner.
#[async_trait]
pub trait Snapshots: Send + Sync {
    async fn capture(&self) -> Option<String>;
    /// Changed files between two snapshots. Failures become `None` (baseline `Effect.catch`).
    async fn files(&self, from: &str, to: &str) -> Option<Vec<RelativePath>>;
}

/// No snapshot support for this Location (non-git directories).
pub struct NoSnapshots;

#[async_trait]
impl Snapshots for NoSnapshots {
    async fn capture(&self) -> Option<String> {
        None
    }
    async fn files(&self, _: &str, _: &str) -> Option<Vec<RelativePath>> {
        None
    }
}
