//! Purpose: ports the legacy Session paths call for capabilities owned by other phases: legacy
//! Agent registry, Provider model lookup, the legacy model loop (`SessionPrompt.runLoop`), the Read
//! tool, MCP resources, LSP symbols, image normalization, plugin hooks, commands, share and
//! process spawning. Each has an `Unwired*` adapter that fails with typed `NotYetPorted`.
//! Scope: object-safe async traits plus unwired adapters.
//! Uses:
//! rust/crates/
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/resource.rs
//! |-- session-runtime/src/title.rs
//! |-- session-runtime/tests/legacy_command.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/title.rs
//! Invariants: unwired adapters never return a fabricated success; they fail with
//! `LegacyError::NotYetPorted` naming the missing baseline capability.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use async_trait::async_trait;
use serde_json::Value;

use super::types::{AgentInfo, LegacyError, ModelRef, Object, WithParts};

/// Legacy `Agent.Service` (packages/opencode/src/agent/agent.ts:64-82).
#[async_trait]
pub trait LegacyAgents: Send + Sync {
    async fn get(&self, name: &str) -> Result<Option<AgentInfo>, LegacyError>;
    async fn list(&self) -> Result<Vec<AgentInfo>, LegacyError>;
    async fn default_info(&self) -> Result<AgentInfo, LegacyError>;
}

/// Legacy `Provider.Service` reads used by prompt/command/title/compaction.
#[async_trait]
pub trait LegacyProvider: Send + Sync {
    /// `provider.getModel`: the `Provider.Model` JSON, or `LegacyError::ModelNotFound`.
    async fn get_model(&self, provider_id: &str, model_id: &str) -> Result<Object, LegacyError>;
    /// `provider.defaultModel()`.
    async fn default_model(&self) -> Result<ModelRef, LegacyError>;
}

/// The legacy provider-turn loop (`SessionPrompt.runLoop`, processor.ts, llm.ts, tools). Dropping
/// the future interrupts the run.
#[async_trait]
pub trait LegacyLoop: Send + Sync {
    async fn run(&self, session_id: &str) -> Result<WithParts, LegacyError>;
}

/// Calling context handed to the Read tool for prompt file parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadContext {
    pub session_id: String,
    pub message_id: String,
    pub agent: Option<String>,
    /// `extra.model` (the resolved `Provider.Model`) for text reads.
    pub model: Option<Object>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadOutput {
    pub output: String,
    /// `FilePart`-shaped attachments.
    pub attachments: Vec<Object>,
}

/// Failure of a delegated capability: a baseline-visible failure message, or a missing port.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    #[error("{0}")]
    Failed(String),
    #[error("not yet ported: {0}")]
    NotYetPorted(&'static str),
}

impl From<PortError> for LegacyError {
    fn from(error: PortError) -> Self {
        match error {
            PortError::Failed(message) => Self::Unknown(message),
            PortError::NotYetPorted(what) => Self::NotYetPorted(what),
        }
    }
}

/// Prompt-time collaborators: Read tool, MCP resources, LSP symbols, images and plugin hooks.
#[async_trait]
pub trait PromptTools: Send + Sync {
    /// `registry.named().read.execute(args, ctx)` with `bypassCwdCheck`.
    async fn read(&self, args: Value, ctx: ReadContext) -> Result<ReadOutput, PortError>;
    /// `mcp.readResource(clientName, uri)`: the resource content (`{ contents }`) or `None`.
    async fn read_resource(&self, client: &str, uri: &str) -> Result<Option<Value>, PortError>;
    /// `lsp.documentSymbol(uri)`. Failures become `[]` at the call site, like the baseline.
    async fn document_symbols(&self, uri: &str) -> Result<Vec<Value>, PortError>;
    /// `image.normalize(part)`. `Ok(None)` is `ResizerUnavailableError` (part kept unchanged).
    async fn normalize_image(&self, part: &Object) -> Result<Option<Object>, PortError>;
    /// `plugin.trigger(name, input, output)`: returns the possibly mutated output.
    async fn trigger(&self, name: &str, input: Value, output: Value) -> Result<Value, PortError>;
}

/// No plugins loaded and no Read/MCP/LSP/image adapters composed.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnwiredPromptTools;

#[async_trait]
impl PromptTools for UnwiredPromptTools {
    async fn read(&self, _: Value, _: ReadContext) -> Result<ReadOutput, PortError> {
        Err(PortError::NotYetPorted(
            "Read tool (p05 tools) for prompt file parts",
        ))
    }
    async fn read_resource(&self, _: &str, _: &str) -> Result<Option<Value>, PortError> {
        Err(PortError::NotYetPorted(
            "MCP.readResource (p07 integrations)",
        ))
    }
    async fn document_symbols(&self, _: &str) -> Result<Vec<Value>, PortError> {
        Err(PortError::NotYetPorted("LSP.documentSymbol"))
    }
    async fn normalize_image(&self, _: &Object) -> Result<Option<Object>, PortError> {
        Ok(None)
    }
    async fn trigger(&self, _: &str, _: Value, output: Value) -> Result<Value, PortError> {
        Ok(output)
    }
}

/// `Command.Info` (packages/opencode/src/command/index.ts:22-34) with a resolved template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub description: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub template: String,
    pub subtask: Option<bool>,
    pub hints: Vec<String>,
}

/// Legacy `Command.Service` plus the config reads `command` needs.
#[async_trait]
pub trait LegacyCommands: Send + Sync {
    async fn get(&self, name: &str) -> Result<Option<CommandInfo>, LegacyError>;
    async fn list(&self) -> Result<Vec<CommandInfo>, LegacyError>;
}

/// `ShareNext` remote calls (p07 / hosted).
#[async_trait]
pub trait ShareRemote: Send + Sync {
    async fn create(&self, session_id: &str) -> Result<String, PortError>;
    async fn remove(&self, session_id: &str) -> Result<(), PortError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnwiredShare;

#[async_trait]
impl ShareRemote for UnwiredShare {
    async fn create(&self, _: &str) -> Result<String, PortError> {
        Err(PortError::NotYetPorted(
            "ShareNext.create (hosted share API)",
        ))
    }
    async fn remove(&self, _: &str) -> Result<(), PortError> {
        Err(PortError::NotYetPorted(
            "ShareNext.remove (hosted share API)",
        ))
    }
}

/// The unported legacy loop: every reply-producing prompt fails typed.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnwiredLoop;

#[async_trait]
impl LegacyLoop for UnwiredLoop {
    async fn run(&self, _: &str) -> Result<WithParts, LegacyError> {
        Err(LegacyError::NotYetPorted(
            "SessionPrompt.runLoop (legacy processor/llm/tool loop)",
        ))
    }
}
