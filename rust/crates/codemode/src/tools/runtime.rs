//! Purpose: per-execution tool runtime: path resolution, namespace enumeration, call admission
//! (argument boundary, input decoding, call budget, start hook) and host execution with output
//! decoding, sanitization and end hooks.
//! Scope: `tool-runtime.ts` resolve/namespaceKeys/make (lines 645-806).
//! Uses:
//! rust/crates/
//! |-- codemode/src/clock.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/tools/discovery.rs
//! |-- codemode/src/tools/mod.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- serde_json::Value
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/pool.rs
//! |-- codemode/src/run.rs
//! `-- codemode/src/tools/mod.rs
//! Invariants: host failure detail never reaches the program; admission errors reject the call's
//! promise rather than the call expression.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/promise.rs, codemode/tests/enumeration.rs

use super::discovery::{RESERVED_NAMESPACE, SearchEntry, search_tool};
use super::{Definition, EndHook, SchemaType, StartHook, ToolCall, ToolCallContext, ToolCallEnded};
use super::{ToolCallOutcome, ToolCallStarted, ToolError, ToolNode, ToolRun, ToolTree};
use crate::error::{DiagnosticKind, Eval, Fault, tool_runtime};
use crate::value::convert::{from_json, is_blocked_member, to_json};
use crate::value::{Value, lock};
use serde_json::Value as Json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub(crate) struct ToolRuntime {
    tree: ToolTree,
    max_tool_calls: Option<usize>,
    pub calls: Mutex<Vec<ToolCall>>,
    on_start: Option<StartHook>,
    on_end: Option<EndHook>,
    pub cancel: CancellationToken,
}

/// An admitted call ready to run on the host.
pub(crate) struct Admitted {
    index: usize,
    name: String,
    input: Json,
    run: Arc<dyn ToolRun>,
    output: Option<SchemaType>,
}

impl ToolRuntime {
    pub fn new(
        tree: &ToolTree,
        max_tool_calls: Option<usize>,
        search_index: Arc<Vec<SearchEntry>>,
        on_start: Option<StartHook>,
        on_end: Option<EndHook>,
        cancel: CancellationToken,
    ) -> Self {
        let mut callable = tree.clone();
        callable.insert(
            RESERVED_NAMESPACE,
            ToolNode::Namespace(ToolTree::new().tool("search", search_tool(search_index))),
        );
        Self {
            tree: callable,
            max_tool_calls,
            calls: Mutex::new(Vec::new()),
            on_start,
            on_end,
            cancel,
        }
    }

    /// Enumerable names at one node of the callable tool tree (`namespaceKeys`).
    pub fn keys(&self, path: &[String]) -> Eval<Vec<String>> {
        let unknown = || {
            tool_runtime(
                DiagnosticKind::UnknownTool,
                format!("Unknown tool namespace '{}'.", path.join(".")),
                &[
                    "Object.keys(tools) lists the available namespaces; tools.$codemode.search({ query }) finds described tools.",
                ],
            )
        };
        let mut tree = &self.tree;
        for (position, segment) in path.iter().enumerate() {
            if is_blocked_member(segment) {
                return Err(unknown());
            }
            match tree.0.get(segment) {
                Some(ToolNode::Namespace(child)) => tree = child,
                Some(_) if position + 1 == path.len() => return Ok(Vec::new()),
                _ => return Err(unknown()),
            }
        }
        Ok(tree.0.keys().cloned().collect())
    }

    fn resolve(&self, path: &[String]) -> Eval<ToolNode> {
        let name = path.join(".");
        let unknown = || {
            tool_runtime(
                DiagnosticKind::UnknownTool,
                format!("Unknown tool '{name}'."),
                &["Use tools.$codemode.search({ query }) to find available described tools."],
            )
        };
        let mut tree = &self.tree;
        let mut node: Option<&ToolNode> = None;
        for segment in path {
            if is_blocked_member(segment)
                || node.is_some_and(|node| !matches!(node, ToolNode::Namespace(_)))
            {
                return Err(unknown());
            }
            let next = tree.0.get(segment).ok_or_else(unknown)?;
            if let ToolNode::Namespace(child) = next {
                tree = child;
            }
            node = Some(next);
        }
        match node {
            Some(ToolNode::Namespace(_)) | None => Err(tool_runtime(
                DiagnosticKind::UnknownTool,
                format!("Tool '{name}' is not callable."),
                &[],
            )),
            Some(other) => Ok(other.clone()),
        }
    }

    /// Admission: argument boundary, resolution, input decoding, call budget, start hook.
    pub fn admit(&self, path: &[String], args: &[Value]) -> Eval<Admitted> {
        let name = path.join(".");
        let label = format!("Arguments for tool '{name}'");
        let external = args
            .iter()
            .map(|arg| Ok(to_json(arg, &label, false)?.unwrap_or(Json::Null)))
            .collect::<Eval<Vec<Json>>>()?;
        let (input, run, output) = match self.resolve(path)? {
            ToolNode::Tool(definition) => {
                if external.len() != 1 {
                    return Err(tool_runtime(
                        DiagnosticKind::InvalidToolInput,
                        format!("Tool '{name}' expects exactly one input object."),
                        &[],
                    ));
                }
                let input = decode_input(&definition, &external[0]).map_err(|cause| {
                    tool_runtime(
                        DiagnosticKind::InvalidToolInput,
                        format!("Invalid input for tool '{name}': {cause}"),
                        &[],
                    )
                })?;
                (input, definition.run.clone(), definition.output.clone())
            }
            ToolNode::Function(function) => (Json::Array(external), function.0.clone(), None),
            ToolNode::Namespace(_) => {
                return Err(tool_runtime(
                    DiagnosticKind::UnknownTool,
                    format!("Tool '{name}' is not callable."),
                    &[],
                ));
            }
        };
        let index = {
            let mut calls = lock(&self.calls);
            if self
                .max_tool_calls
                .is_some_and(|limit| calls.len() >= limit)
            {
                return Err(tool_runtime(
                    DiagnosticKind::ToolCallLimitExceeded,
                    format!(
                        "Execution exceeded its tool-call limit of {}.",
                        self.max_tool_calls.unwrap_or_default()
                    ),
                    &[],
                ));
            }
            calls.push(ToolCall { name: name.clone() });
            calls.len() - 1
        };
        if let Some(hook) = &self.on_start {
            hook(&ToolCallStarted {
                index,
                name: name.clone(),
                input: input.clone(),
            });
        }
        Ok(Admitted {
            index,
            name,
            input,
            run,
            output,
        })
    }

    /// Runs an admitted call on the host and decodes its output into the sandbox.
    pub async fn run(self: Arc<Self>, admitted: Admitted) -> Eval<Value> {
        let started = crate::clock::Stopwatch::start();
        let context = ToolCallContext {
            cancel: self.cancel.child_token(),
        };
        let host = admitted.run.run(admitted.input.clone(), context).await;
        let result = match host {
            Err(ToolError::Interrupted) => return Err(Fault::Interrupted),
            Err(ToolError::Refused(message)) => Err(Fault::Tool(message)),
            Err(ToolError::Internal(_)) => Err(Fault::Tool("Tool execution failed".into())),
            Ok(raw) => decode_output(&admitted, &raw),
        };
        if let Some(hook) = &self.on_end {
            let (outcome, message) = match &result {
                Ok(_) => (ToolCallOutcome::Success, None),
                Err(Fault::Tool(message)) | Err(Fault::ToolRuntime { message, .. }) => {
                    (ToolCallOutcome::Failure, Some(message.clone()))
                }
                Err(_) => (
                    ToolCallOutcome::Failure,
                    Some("Tool execution failed".into()),
                ),
            };
            hook(&ToolCallEnded {
                index: admitted.index,
                name: admitted.name.clone(),
                input: admitted.input.clone(),
                duration_ms: started.elapsed_ms(),
                outcome,
                message,
            });
        }
        result
    }
}

fn decode_input(definition: &Definition, value: &Json) -> Result<Json, String> {
    match &definition.input {
        SchemaType::Validating(schema) => schema.decode(value),
        SchemaType::Json(_) => Ok(value.clone()),
    }
}

fn decode_output(admitted: &Admitted, raw: &Json) -> Eval<Value> {
    let invalid = || {
        tool_runtime(
            DiagnosticKind::InvalidToolOutput,
            format!("Invalid output from tool '{}'.", admitted.name),
            &[],
        )
    };
    let decoded = match &admitted.output {
        Some(SchemaType::Validating(schema)) => schema.decode(raw).map_err(|_| invalid())?,
        _ => raw.clone(),
    };
    from_json(&decoded, &format!("Result from tool '{}'", admitted.name)).map_err(|_| invalid())
}
