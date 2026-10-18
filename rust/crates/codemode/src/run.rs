//! Purpose: public CodeMode API: execution options and limits, the reusable runtime, results and
//! diagnostics, timeout/cancellation, and model-facing output bounding.
//! Scope: codemode.ts plus runtime.ts executeWithLimits/boundOutput (lines 3360-3465).
//! Uses:
//! rust/crates/
//! |-- codemode/src/clock.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/parse/mod.rs
//! |-- codemode/src/tools/discovery.rs
//! |-- codemode/src/tools/mod.rs
//! `-- codemode/src/tools/runtime.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- codemode/src/lib.rs
//! Invariants: program failures are data (`Result::Failure`); only host interruption surfaces as
//! `Err(Interrupted)`; output never exceeds `max_output_bytes` (plus markers) when set.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/promise.rs

use crate::error::{DiagnosticKind, Fault};
use crate::interp::diagnose::normalize;
use crate::interp::{Interp, pool::Pool};
use crate::tools::discovery::{
    DEFAULT_CATALOG_BUDGET, RESERVED_NAMESPACE, SearchEntry, prepare, search_index,
};
use crate::tools::runtime::ToolRuntime;
use crate::tools::{EndHook, StartHook, ToolCall, ToolDescription, ToolTree};
use crate::value::{convert, lock};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Resource budgets enforced per execution. `None` means unlimited.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExecutionLimits {
    pub timeout_ms: Option<f64>,
    pub max_tool_calls: Option<f64>,
    pub max_output_bytes: Option<f64>,
}

/// Invalid configuration (`RangeError` in the baseline).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} must be a safe integer greater than or equal to {1}.")]
    InvalidLimit(&'static str, u32),
    #[error("discovery.catalogBudget must be a non-negative safe integer")]
    InvalidCatalogBudget,
    #[error("Tool namespace '$codemode' is reserved for CodeMode discovery tools.")]
    ReservedNamespace,
}

/// The execution was interrupted by the host (cancellation), not by a program failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("CodeMode execution was interrupted")]
pub struct Interrupted;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub line: u32,
    pub column: u32,
}

/// A normalized program diagnostic safe to return across an agent tool boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggestions: Option<Vec<String>>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            location: None,
            suggestions: None,
        }
    }
}

/// `CodeMode.Result`: success or diagnostic, with captured logs and admitted calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionResult {
    pub ok: bool,
    /// Present (possibly `null`) exactly when `ok`; absent on failures.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    pub value: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub logs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub truncated: Option<bool>,
    pub tool_calls: Vec<ToolCall>,
}

/// Keeps a present JSON `null` as `Some(Null)` (serde maps it to `None` by default).
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Json>, D::Error> {
    Json::deserialize(deserializer).map(Some)
}

/// Options shared by [`execute`] and [`Runtime::new`].
#[derive(Clone, Default)]
pub struct Options {
    pub tools: ToolTree,
    pub limits: ExecutionLimits,
    /// Approximate token budget (chars/4, default 2000) for inlined catalog entries.
    pub catalog_budget: Option<f64>,
    pub on_tool_call_start: Option<StartHook>,
    pub on_tool_call_end: Option<EndHook>,
}

#[derive(Clone, Copy)]
struct Resolved {
    timeout_ms: Option<u64>,
    max_tool_calls: Option<usize>,
    max_output_bytes: Option<usize>,
}

fn validate(
    name: &'static str,
    value: Option<f64>,
    minimum: u32,
) -> Result<Option<u64>, ConfigError> {
    match value {
        None => Ok(None),
        Some(value)
            if crate::value::number::is_safe_integer(value) && value >= f64::from(minimum) =>
        {
            Ok(Some(value as u64))
        }
        Some(_) => Err(ConfigError::InvalidLimit(name, minimum)),
    }
}

fn resolve_limits(limits: &ExecutionLimits) -> Result<Resolved, ConfigError> {
    Ok(Resolved {
        timeout_ms: validate("timeoutMs", limits.timeout_ms, 1)?,
        max_tool_calls: validate("maxToolCalls", limits.max_tool_calls, 0)?
            .map(|value| value as usize),
        max_output_bytes: validate("maxOutputBytes", limits.max_output_bytes, 0)?
            .map(|value| value as usize),
    })
}

fn assert_valid_tools(tools: &ToolTree) -> Result<(), ConfigError> {
    if tools.0.contains_key(RESERVED_NAMESPACE) {
        return Err(ConfigError::ReservedNamespace);
    }
    Ok(())
}

/// Reusable confined runtime over one explicit tool tree.
#[derive(Clone)]
pub struct Runtime {
    options: Options,
    limits: Resolved,
    catalog: Vec<ToolDescription>,
    instructions: String,
    search_index: Arc<Vec<SearchEntry>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Runtime")
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// `CodeMode.make(options)`.
    pub fn new(options: Options) -> Result<Self, ConfigError> {
        assert_valid_tools(&options.tools)?;
        let limits = resolve_limits(&options.limits)?;
        let budget = match options.catalog_budget {
            None => DEFAULT_CATALOG_BUDGET,
            Some(budget) if crate::value::number::is_safe_integer(budget) && budget >= 0.0 => {
                budget as usize
            }
            Some(_) => return Err(ConfigError::InvalidCatalogBudget),
        };
        let plan = prepare(&options.tools, budget);
        Ok(Self {
            options,
            limits,
            catalog: plan.catalog,
            instructions: plan.instructions,
            search_index: Arc::new(plan.search_index),
        })
    }

    pub fn catalog(&self) -> &[ToolDescription] {
        &self.catalog
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// Executes one program. Host cancellation of `cancel` yields `Err(Interrupted)`.
    pub async fn execute(
        &self,
        code: &str,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, Interrupted> {
        run(
            code,
            &self.options,
            self.limits,
            self.search_index.clone(),
            cancel,
        )
        .await
    }
}

/// `CodeMode.execute(options)`: one-shot execution without a reusable runtime.
pub async fn execute(
    code: &str,
    options: &Options,
    cancel: CancellationToken,
) -> Result<Result<ExecutionResult, Interrupted>, ConfigError> {
    assert_valid_tools(&options.tools)?;
    let limits = resolve_limits(&options.limits)?;
    Ok(run(
        code,
        options,
        limits,
        Arc::new(search_index(&options.tools)),
        cancel,
    )
    .await)
}

enum Outcome {
    Done(Result<Json, Fault>),
    TimedOut,
    Cancelled,
}

async fn run(
    code: &str,
    options: &Options,
    limits: Resolved,
    index: Arc<Vec<SearchEntry>>,
    cancel: CancellationToken,
) -> Result<ExecutionResult, Interrupted> {
    let execution = cancel.child_token();
    let tools = Arc::new(ToolRuntime::new(
        &options.tools,
        limits.max_tool_calls,
        index,
        options.on_tool_call_start.clone(),
        options.on_tool_call_end.clone(),
        execution.clone(),
    ));
    let calls = || lock(&tools.calls).clone();
    if code.trim().is_empty() {
        return Ok(failure(
            Diagnostic::new(DiagnosticKind::ParseError, "Code cannot be empty."),
            Vec::new(),
            calls(),
        ));
    }
    let mut interp = Interp::new(Pool::new(tools.clone()));
    let outcome = {
        let program = async {
            let program = crate::parse::parse_program(code).map_err(|message| {
                let mut error = crate::error::runtime(message, None);
                error.kind = DiagnosticKind::ParseError;
                Fault::Runtime(error)
            })?;
            let value = interp.run(&program).await?;
            Ok(convert::to_json(&value, "Execution result", true)?.unwrap_or(Json::Null))
        }
        .fuse();
        let timer = match limits.timeout_ms.and_then(crate::clock::sleep) {
            Some(timer) => timer.fuse(),
            None => futures::future::pending::<()>().boxed().fuse(),
        };
        let cancelled = cancel.cancelled().fuse();
        futures::pin_mut!(program, timer, cancelled);
        futures::select_biased! {
            result = program => Outcome::Done(result),
            () = timer => Outcome::TimedOut,
            () = cancelled => Outcome::Cancelled,
        }
    };
    // Dropping the interpreter drops every in-flight host future; cancel tells hosts too.
    execution.cancel();
    let logs = std::mem::take(&mut interp.logs);
    drop(interp);
    let result = match outcome {
        Outcome::Cancelled | Outcome::Done(Err(Fault::Interrupted)) => return Err(Interrupted),
        Outcome::TimedOut => failure(
            Diagnostic::new(
                DiagnosticKind::TimeoutExceeded,
                format!(
                    "Execution timed out after {}ms.",
                    limits.timeout_ms.unwrap_or_default()
                ),
            ),
            logs,
            calls(),
        ),
        Outcome::Done(Ok(value)) => ExecutionResult {
            ok: true,
            value: Some(value),
            error: None,
            logs: (!logs.is_empty()).then_some(logs),
            truncated: None,
            tool_calls: calls(),
        },
        Outcome::Done(Err(fault)) => failure(normalize(&fault), logs, calls()),
    };
    Ok(match limits.max_output_bytes {
        Some(max) => crate::output::bound(result, max),
        None => result,
    })
}

fn failure(error: Diagnostic, logs: Vec<String>, tool_calls: Vec<ToolCall>) -> ExecutionResult {
    ExecutionResult {
        ok: false,
        value: None,
        error: Some(error),
        logs: (!logs.is_empty()).then_some(logs),
        truncated: None,
        tool_calls,
    }
}
