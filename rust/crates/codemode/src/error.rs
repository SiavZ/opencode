//! Purpose: interpreter fault model (runtime errors, tool-runtime errors, safe tool errors,
//! program throws, interruption) and the public diagnostic vocabulary.
//! Scope: error values and constructors; normalization lives in `interp/diagnose.rs`.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/mod.rs
//! `-- codemode/src/tools/runtime.rs
//! Invariants: `DiagnosticKind` serializes with the exact TypeScript literal names.
//! Contract tests: codemode/tests/codemode.rs

use crate::ast::Loc;
use crate::value::Value;
use serde::{Deserialize, Serialize};

/// Stable categories produced by program, schema, tool, and limit failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    ParseError,
    UnsupportedSyntax,
    UnknownTool,
    InvalidToolInput,
    InvalidToolOutput,
    InvalidDataValue,
    ToolCallLimitExceeded,
    TimeoutExceeded,
    ToolFailure,
    ExecutionFailure,
}

pub(crate) const SUPPORTED_SYNTAX_MESSAGE: &str = "Supported orchestration syntax: tools.* calls (they return promises - resolve them with await), data literals, destructuring, optional chaining, template literals, conditionals, switch, loops (incl. for...of and for...in over object/array/tools keys), arrow functions, spread, try/catch, array methods (map/filter/find/findIndex/some/every/reduce/flatMap/forEach/sort/slice/concat/indexOf/lastIndexOf/at/flat/reverse/includes/join), string methods (incl. match/matchAll/replace/split with regular expressions), Date/RegExp/Map/Set/URL/URLSearchParams, URI encoding helpers, Object/Math/JSON helpers, captured console.log/warn/error/dir/table, and Promise.all/allSettled/race/resolve/reject over arrays mixing promises and plain values for parallel tool calls (promise chaining with .then/.catch is not supported - use await with try/catch).";

/// `InterpreterRuntimeError`: a diagnostic raised by the interpreter itself.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeError {
    pub message: String,
    pub loc: Option<Loc>,
    pub kind: DiagnosticKind,
    pub suggestions: Option<Vec<String>>,
    /// JS constructor name the program observes when catching it (`.as(...)`).
    pub error_name: &'static str,
}

/// Every way evaluation can stop abruptly other than normal completion.
#[derive(Clone, Debug)]
pub(crate) enum Fault {
    Runtime(RuntimeError),
    /// `ToolRuntimeError`: boundary validation and tool resolution failures.
    ToolRuntime {
        kind: DiagnosticKind,
        message: String,
        suggestions: Vec<String>,
    },
    /// `ToolError`: a safe operational refusal from a tool; the message is model-visible.
    Tool(String),
    /// `throw value` from the program.
    Throw(ValueBox),
    /// Execution teardown (timeout, host cancellation). Never program-visible.
    Interrupted,
}

/// `Value` is not `Debug`; wrap it so `Fault` stays debuggable.
#[derive(Clone)]
pub(crate) struct ValueBox(pub Value);

impl std::fmt::Debug for ValueBox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Value")
    }
}

pub(crate) type Eval<T> = Result<T, Fault>;

impl RuntimeError {
    pub fn named(mut self, name: &'static str) -> Self {
        self.error_name = name;
        self
    }
}

impl From<RuntimeError> for Fault {
    fn from(error: RuntimeError) -> Self {
        Fault::Runtime(error)
    }
}

/// `new InterpreterRuntimeError(message, node)` with the default ExecutionFailure kind.
pub(crate) fn runtime(message: impl Into<String>, loc: Option<Loc>) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        loc,
        kind: DiagnosticKind::ExecutionFailure,
        suggestions: None,
        error_name: "Error",
    }
}

pub(crate) fn fail(message: impl Into<String>, loc: Loc) -> Fault {
    Fault::Runtime(runtime(message, Some(loc)))
}

pub(crate) fn fail_kind(message: impl Into<String>, loc: Loc, kind: DiagnosticKind) -> Fault {
    let mut error = runtime(message, Some(loc));
    error.kind = kind;
    Fault::Runtime(error)
}

pub(crate) fn fail_named(message: impl Into<String>, loc: Loc, name: &'static str) -> Fault {
    Fault::Runtime(runtime(message, Some(loc)).named(name))
}

/// Unsupported construct that advertises the supported syntax subset.
pub(crate) fn unsupported_feature(message: impl Into<String>, loc: Loc) -> Fault {
    let mut error = runtime(message, Some(loc));
    error.kind = DiagnosticKind::UnsupportedSyntax;
    error.suggestions = Some(vec![SUPPORTED_SYNTAX_MESSAGE.to_string()]);
    Fault::Runtime(error)
}

/// `unsupportedSyntax(kind, node)`.
pub(crate) fn unsupported_syntax(kind: &str, loc: Loc) -> Fault {
    unsupported_feature(
        format!("Syntax '{kind}' is not supported in CodeMode. {SUPPORTED_SYNTAX_MESSAGE}"),
        loc,
    )
}

pub(crate) fn data_error(message: impl Into<String>) -> Fault {
    Fault::ToolRuntime {
        kind: DiagnosticKind::InvalidDataValue,
        message: message.into(),
        suggestions: Vec::new(),
    }
}

pub(crate) fn tool_runtime(
    kind: DiagnosticKind,
    message: impl Into<String>,
    suggestions: &[&str],
) -> Fault {
    Fault::ToolRuntime {
        kind,
        message: message.into(),
        suggestions: suggestions.iter().map(|item| (*item).to_string()).collect(),
    }
}
