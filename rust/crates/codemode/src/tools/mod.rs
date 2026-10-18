//! Purpose: public tool contract: schema-described definitions, the host tool tree, safe tool
//! errors, call observation hooks, and the async host bridge trait.
//! Scope: types and constructors; resolution/admission lives in `tools/runtime.rs`.
//! Uses:
//! rust/crates/
//! |-- codemode/src/tools/discovery.rs
//! |-- codemode/src/tools/render.rs
//! |-- codemode/src/tools/runtime.rs
//! `-- codemode/src/tools/schema.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- serde_json::Value
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/src/openapi/mod.rs
//! Invariants: `ToolError::Refused` messages are model-visible; every other host failure is
//! reported as "Tool execution failed" with its detail dropped.
//! Contract tests: codemode/tests/codemode.rs

pub(crate) mod discovery;
pub(crate) mod render;
pub(crate) mod runtime;
pub mod schema;
pub(crate) mod text;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use schema::Schema;
use serde_json::Value as Json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub use render::{input_type_script, json_schema_to_type_script, output_type_script};

/// Safe operational refusal (model-visible) or a sanitized internal failure.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ToolError {
    /// `ToolError`: a refusal whose message is safe to include in a diagnostic.
    #[error("{0}")]
    Refused(String),
    /// Any other host failure or defect. The detail never reaches the program or diagnostic.
    #[error("Tool execution failed")]
    Internal(String),
    /// The host cancelled the call; propagates as execution interruption.
    #[error("Tool call interrupted")]
    Interrupted,
}

/// `toolError(message)`.
pub fn tool_error(message: impl Into<String>) -> ToolError {
    ToolError::Refused(message.into())
}

/// Per-call context handed to host tools.
#[derive(Clone, Debug)]
pub struct ToolCallContext {
    /// Cancelled when the execution times out or the host cancels it.
    pub cancel: CancellationToken,
}

pub type ToolFuture = BoxFuture<'static, Result<Json, ToolError>>;

/// Async host bridge for one tool. Implemented for closures `Fn(Json, ToolCallContext) -> ToolFuture`.
pub trait ToolRun: Send + Sync {
    fn run(&self, input: Json, context: ToolCallContext) -> ToolFuture;
}

impl<F> ToolRun for F
where
    F: Fn(Json, ToolCallContext) -> ToolFuture + Send + Sync,
{
    fn run(&self, input: Json, context: ToolCallContext) -> ToolFuture {
        self(input, context)
    }
}

/// Either a validating schema or a render-only JSON Schema document.
#[derive(Clone, Debug)]
pub enum SchemaType {
    Validating(Schema),
    Json(Json),
}

impl From<Schema> for SchemaType {
    fn from(schema: Schema) -> Self {
        Self::Validating(schema)
    }
}

impl From<Json> for SchemaType {
    fn from(schema: Json) -> Self {
        Self::Json(schema)
    }
}

/// `Tool.Definition`: a schema-described tool.
#[derive(Clone)]
pub struct Definition {
    pub description: String,
    pub input: SchemaType,
    pub output: Option<SchemaType>,
    pub run: Arc<dyn ToolRun>,
}

impl std::fmt::Debug for Definition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Definition")
            .field("description", &self.description)
            .field("input", &self.input)
            .field("output", &self.output)
            .finish_non_exhaustive()
    }
}

impl Definition {
    /// `Tool.make({ description, input, output?, run })`.
    pub fn new(
        description: impl Into<String>,
        input: impl Into<SchemaType>,
        output: Option<SchemaType>,
        run: impl ToolRun + 'static,
    ) -> Self {
        Self {
            description: description.into(),
            input: input.into(),
            output,
            run: Arc::new(run),
        }
    }
}

/// A raw host function tool (`(...args) => Effect`): receives the argument list as a JSON array.
pub type HostFunction = Arc<dyn ToolRun>;

/// A node of the explicit tool tree exposed to programs as `tools`.
#[derive(Clone, Debug)]
pub enum ToolNode {
    Tool(Definition),
    Function(HostFunctionNode),
    Namespace(ToolTree),
}

#[derive(Clone)]
pub struct HostFunctionNode(pub HostFunction);

impl std::fmt::Debug for HostFunctionNode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostFunction")
    }
}

/// Insertion-ordered tool namespace (`{ [name]: Definition | HostTool | ToolTree }`).
#[derive(Clone, Debug, Default)]
pub struct ToolTree(pub IndexMap<String, ToolNode>);

impl ToolTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool(mut self, name: impl Into<String>, definition: Definition) -> Self {
        self.0.insert(name.into(), ToolNode::Tool(definition));
        self
    }

    pub fn function(mut self, name: impl Into<String>, run: impl ToolRun + 'static) -> Self {
        self.0.insert(
            name.into(),
            ToolNode::Function(HostFunctionNode(Arc::new(run))),
        );
        self
    }

    pub fn namespace(mut self, name: impl Into<String>, tree: ToolTree) -> Self {
        self.0.insert(name.into(), ToolNode::Namespace(tree));
        self
    }

    pub fn insert(&mut self, name: impl Into<String>, node: ToolNode) {
        self.0.insert(name.into(), node);
    }

    /// Looks up a node by dotted path segments.
    pub fn get(&self, path: &[&str]) -> Option<&ToolNode> {
        let (head, rest) = path.split_first()?;
        let node = self.0.get(*head)?;
        if rest.is_empty() {
            return Some(node);
        }
        match node {
            ToolNode::Namespace(tree) => tree.get(rest),
            _ => None,
        }
    }
}

/// Minimal audit record retained for each admitted tool call.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub name: String,
}

/// Decoded tool call observed immediately before tool execution.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallStarted {
    pub index: usize,
    pub name: String,
    pub input: Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCallOutcome {
    Success,
    Failure,
}

/// Completed tool call observed immediately after tool execution settles.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallEnded {
    pub index: usize,
    pub name: String,
    pub input: Json,
    pub duration_ms: f64,
    pub outcome: ToolCallOutcome,
    /// Model-safe failure message; present only on failure.
    pub message: Option<String>,
}

pub type StartHook = Arc<dyn Fn(&ToolCallStarted) + Send + Sync>;
pub type EndHook = Arc<dyn Fn(&ToolCallEnded) + Send + Sync>;

/// Model-visible description of one schema-backed tool.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolDescription {
    pub path: String,
    pub description: String,
    pub signature: String,
}
