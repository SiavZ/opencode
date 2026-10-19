//! Purpose: typed parse result the dispatcher consumes: resolved command, yargs-shaped argument
//! values, global flags, `--` passthrough and the non-run outcomes (help, version, completion, failure).
//! Scope: one process invocation.
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/argv.rs
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! Invariants: argument keys are canonical kebab-case option/positional names; values follow
//! yargs-parser coercion (number NaN is `Null`, repeated scalars become `List`).
//! Contract tests: opencode/tests/cli_parse.rs
use std::collections::BTreeMap;

pub use super::model::CommandId;

/// A parsed yargs value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Str(String),
    Num(f64),
    /// `Number(x)` was NaN; yargs reports `null`.
    Null,
    List(Vec<Value>),
}

impl Value {
    /// Last scalar of a repeated option, or the value itself.
    pub fn last(&self) -> &Value {
        match self {
            Value::List(items) if !items.is_empty() => items[items.len() - 1].last(),
            v => v,
        }
    }
}

/// Global options from packages/opencode/src/index.ts:46-65.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Globals {
    pub print_logs: bool,
    pub log_level: Option<String>,
    pub pure: bool,
}

impl Globals {
    /// Env the yargs middleware sets (index.ts:66-78), in assignment order.
    pub fn middleware_env(&self, pid: u32) -> Vec<(&'static str, String)> {
        let mut vars = Vec::new();
        if self.print_logs {
            vars.push(("OPENCODE_PRINT_LOGS", "1".to_string()));
        }
        if let Some(level) = self.log_level.as_ref().filter(|l| !l.is_empty()) {
            vars.push(("OPENCODE_LOG_LEVEL", level.clone()));
        }
        if self.pure {
            vars.push(("OPENCODE_PURE", "1".to_string()));
        }
        vars.push(("AGENT", "1".to_string()));
        vars.push(("OPENCODE", "1".to_string()));
        vars.push(("OPENCODE_PID", pid.to_string()));
        vars
    }
}

/// Arguments of the resolved command, keyed by canonical name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub values: BTreeMap<String, Value>,
}

impl Args {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// String value (last when repeated). Numbers render like JS `String(n)`.
    pub fn str(&self, name: &str) -> Option<String> {
        match self.get(name)?.last() {
            Value::Str(s) => Some(s.clone()),
            Value::Num(n) => Some(js_number_string(*n)),
            Value::Bool(b) => Some(b.to_string()),
            Value::Null | Value::List(_) => None,
        }
    }

    pub fn bool(&self, name: &str) -> bool {
        matches!(self.get(name).map(Value::last), Some(Value::Bool(true)))
    }

    /// Explicit boolean, `None` when absent (`args.replay === undefined`).
    pub fn bool_opt(&self, name: &str) -> Option<bool> {
        match self.get(name)?.last() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn num(&self, name: &str) -> Option<f64> {
        match self.get(name)?.last() {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Array option or variadic positional as strings.
    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Some(Value::List(items)) => items.iter().filter_map(scalar_string).collect(),
            Some(v) => scalar_string(v).into_iter().collect(),
            None => Vec::new(),
        }
    }
}

fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Num(n) => Some(js_number_string(*n)),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null | Value::List(_) => None,
    }
}

/// JS `String(number)` for the finite values a CLI produces.
pub fn js_number_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.fract() == 0.0 && n.abs() < 1e21 {
        return format!("{}", n as i64);
    }
    format!("{n}")
}

/// A command ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub command: CommandId,
    /// Canonical command path, e.g. `["mcp", "auth", "list"]`; empty for the default tui command.
    pub path: Vec<&'static str>,
    pub globals: Globals,
    pub args: Args,
    /// Tokens after `--` (`populate--`), used by `run` and `mcp add`.
    pub double_dash: Vec<String>,
    /// Raw argv after the executable (cli/network.ts `hasArg` inspects it).
    pub raw: Vec<String>,
}

/// Every outcome of parsing the legacy command line.
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    Run(Parsed),
    /// Text for stderr and the exit code (help is 0, `completion --help` is 1).
    Help {
        stderr: String,
        code: u8,
    },
    /// Version line for stdout.
    Version(String),
    /// Shell completion script for stdout.
    CompletionScript(String),
    /// `--get-yargs-completions` reply for stdout.
    Completions(String),
    /// Strict/validation failure: help to stderr, exit 1.
    Fail {
        stderr: String,
    },
}
