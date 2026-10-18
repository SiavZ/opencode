//! Purpose: crate root for the confined CodeMode interpreter (packages/codemode port): a restricted
//! JavaScript/TypeScript subset parsed with oxc and evaluated by an owned tree-walking interpreter
//! over explicit, schema-described host tools. No general JS engine is embedded.
//! Scope: module tree and public re-exports.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/clock.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/output.rs
//! |-- codemode/src/parse/mod.rs
//! |-- codemode/src/run.rs
//! |-- codemode/src/stdlib/mod.rs
//! |-- codemode/src/tools/mod.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-07, consumed by integration-adapters and tools)
//! Invariants: portable (native + wasm32), no ambient process/network/filesystem access; host
//! effects only flow through `ToolRun` implementations and the `openapi::HttpClient` capability.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/parity.rs, codemode/tests/promise.rs,
//! codemode/tests/stdlib.rs, codemode/tests/enumeration.rs, codemode/tests/signature.rs,
//! codemode/tests/openapi.rs

mod ast;
mod clock;
mod error;
mod interp;
pub mod openapi;
mod output;
mod parse;
mod run;
mod stdlib;
pub mod tools;
mod value;

pub use error::DiagnosticKind;
pub use run::{
    ConfigError, Diagnostic, ExecutionLimits, ExecutionResult, Interrupted, Location, Options,
    Runtime, execute,
};
pub use tools::schema::Schema;
pub use tools::{
    Definition, SchemaType, ToolCall, ToolCallContext, ToolCallEnded, ToolCallOutcome,
    ToolCallStarted, ToolDescription, ToolError, ToolFuture, ToolNode, ToolRun, ToolTree,
    input_type_script, json_schema_to_type_script, output_type_script, tool_error,
};

/// `copyOut` for JSON numbers at the host boundary: non-finite numbers become `null`.
pub fn copy_out_number(number: f64) -> serde_json::Value {
    value::convert::json_number(number)
}

fn interp_typeof(value: &value::Value) -> &'static str {
    interp::typeof_of(value)
}

fn interp_spread(value: &value::Value) -> Option<Vec<value::Value>> {
    interp::spread_of(value)
}
