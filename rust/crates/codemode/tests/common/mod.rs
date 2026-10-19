//! Purpose: shared harness for the ported CodeMode test suites (fixture-free, live in-process).
//! Scope: execution helpers and tool builders mirroring `Tool.make` usage in the TS tests.
//! Uses:
//! rust/crates/
//! `-- codemode/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- codemode/tests/codemode.rs
//! |-- codemode/tests/enumeration.rs
//! |-- codemode/tests/openapi.rs
//! |-- codemode/tests/parity.rs
//! |-- codemode/tests/promise.rs
//! |-- codemode/tests/signature.rs
//! `-- codemode/tests/stdlib.rs
//! Invariants: helpers only call the public crate API.
//! Contract tests: none, test support

#![allow(dead_code)]

use opencode_codemode::{
    Definition, Diagnostic, ExecutionResult, Options, Runtime, Schema, SchemaType, ToolCallContext,
    ToolError, ToolFuture, ToolTree,
};
use serde_json::Value as Json;
use std::future::Future;
use tokio_util::sync::CancellationToken;

pub async fn run_with(code: &str, options: &Options) -> ExecutionResult {
    opencode_codemode::execute(code, options, CancellationToken::new())
        .await
        .expect("valid configuration")
        .expect("not interrupted")
}

pub async fn run_tools(code: &str, tools: ToolTree) -> ExecutionResult {
    run_with(
        code,
        &Options {
            tools,
            ..Options::default()
        },
    )
    .await
}

pub async fn run(code: &str) -> ExecutionResult {
    run_tools(code, ToolTree::new()).await
}

pub async fn value(code: &str) -> Json {
    let result = run(code).await;
    assert!(
        result.ok,
        "expected success for {code:?}, got {:?}",
        result.error
    );
    result.value.unwrap_or(Json::Null)
}

pub async fn error(code: &str) -> Diagnostic {
    let result = run(code).await;
    assert!(
        !result.ok,
        "expected failure for {code:?}, got value {:?}",
        result.value
    );
    result.error.expect("diagnostic")
}

pub async fn runtime_value(runtime: &Runtime, code: &str) -> ExecutionResult {
    runtime
        .execute(code, CancellationToken::new())
        .await
        .expect("not interrupted")
}

/// `Tool.make({ description, input, output?, run })` with a synchronous body.
pub fn tool(
    description: &str,
    input: impl Into<SchemaType>,
    output: Option<SchemaType>,
    run: impl Fn(Json) -> Result<Json, ToolError> + Send + Sync + 'static,
) -> Definition {
    Definition::new(
        description,
        input,
        output,
        move |input: Json, _context: ToolCallContext| -> ToolFuture {
            let result = run(input);
            Box::pin(async move { result })
        },
    )
}

/// `Tool.make(...)` with an async body.
pub fn async_tool<F, Fut>(
    description: &str,
    input: impl Into<SchemaType>,
    output: Option<SchemaType>,
    run: F,
) -> Definition
where
    F: Fn(Json, ToolCallContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Json, ToolError>> + Send + 'static,
{
    Definition::new(
        description,
        input,
        output,
        move |input: Json, context: ToolCallContext| -> ToolFuture {
            Box::pin(run(input, context))
        },
    )
}

pub fn empty_struct() -> Schema {
    Schema::object(Vec::<(String, Schema)>::new())
}

pub fn ns(entries: Vec<(&str, Definition)>) -> ToolTree {
    entries
        .into_iter()
        .fold(ToolTree::new(), |tree, (name, definition)| {
            tree.tool(name, definition)
        })
}
