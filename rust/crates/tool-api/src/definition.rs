//! Purpose: the canonical tool trait and definition (packages/core/src/tool/tool.ts): description,
//! JSON input/output schemas, permission action override, typed failure, name validation.
//! Scope: contract only.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/context.rs
//! `-- tool-api/src/output.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! `-- tool-api/src/registry.rs
//! Invariants: invalid input fails with `Invalid tool input: ...`; names match
//! `^[A-Za-z][A-Za-z0-9_-]{0,63}$`.
//! Contract tests: tool-api/tests/registry.rs

use crate::context::ToolContext;
use crate::output::ToolOutput;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `LLM.ToolDefinition` wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_schema: Option<Value>,
}

/// `LLM.ToolFailure`: model-visible failure message plus optional metadata.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ToolFailure {
    pub message: String,
    pub metadata: Option<Map<String, Value>>,
}

impl ToolFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            metadata: None,
        }
    }

    pub fn invalid_input(detail: impl std::fmt::Display) -> Self {
        Self::new(format!("Invalid tool input: {detail}"))
    }
}

impl From<opencode_permissions::PermissionError> for ToolFailure {
    fn from(error: opencode_permissions::PermissionError) -> Self {
        Self::new(error.to_string())
    }
}

/// A leaf tool. Implementations decode `input` against their schema and return a typed failure for
/// invalid input instead of panicking.
#[async_trait]
pub trait Tool: Send + Sync {
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn output_schema(&self) -> Option<Value> {
        None
    }
    /// Permission action used for catalog filtering, defaults to the registered name.
    fn permission(&self) -> Option<&str> {
        None
    }
    async fn execute(&self, input: Value, context: &ToolContext)
    -> Result<ToolOutput, ToolFailure>;

    fn definition(&self, name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: self.description().into(),
            input_schema: self.input_schema(),
            output_schema: self.output_schema(),
        }
    }
}

/// `Tool.validateName`.
pub fn validate_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
