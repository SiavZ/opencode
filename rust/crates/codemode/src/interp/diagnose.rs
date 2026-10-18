//! Purpose: diagnostic normalization (`normalizeError`), caught-error values, path redaction.
//! Scope: converting faults into public diagnostics and program-visible error objects.
//! Uses:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/mod.rs
//! `-- codemode/src/run.rs
//! Invariants: host paths are redacted from every message; thrown runtime references never leak.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{DiagnosticKind, Fault};
use crate::run::{Diagnostic, Location};
use crate::value::{ObjectData, Value, convert, lock};

/// Replaces host filesystem paths (`/Users/...`, `/home/...`, `/private/...`, `/tmp/...`,
/// `/var/folders/...`) with `<redacted-path>`.
pub(crate) fn public_message(message: &str) -> String {
    const PREFIXES: [&str; 5] = ["/Users/", "/home/", "/private/", "/tmp/", "/var/folders/"];
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    'scan: while !rest.is_empty() {
        for prefix in PREFIXES {
            if let Some(after) = rest.strip_prefix(prefix) {
                let end = after
                    .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`'))
                    .unwrap_or(after.len());
                if end > 0 {
                    out.push_str("<redacted-path>");
                    rest = &after[end..];
                    continue 'scan;
                }
            }
        }
        let ch = rest.chars().next().unwrap_or_default();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

fn format_location(loc: Option<Loc>) -> String {
    loc.map_or_else(String::new, |loc| {
        format!(" (line {}, col {})", loc.line, loc.column)
    })
}

fn contains_runtime_reference(value: &Value, seen: &mut Vec<usize>) -> bool {
    if value.is_runtime_reference() {
        return true;
    }
    let Some(id) = value.identity() else {
        return false;
    };
    if seen.contains(&id) {
        return false;
    }
    seen.push(id);
    let items: Vec<Value> = match value {
        Value::Array(array) => lock(array).items.clone(),
        Value::Object(object) => lock(object).props.values().cloned().collect(),
        _ => Vec::new(),
    };
    let found = items
        .iter()
        .any(|item| contains_runtime_reference(item, seen));
    seen.pop();
    found
}

pub(crate) fn has_runtime_reference(value: &Value) -> bool {
    contains_runtime_reference(value, &mut Vec::new())
}

/// `normalizeError`: a model-safe diagnostic for any fault except interruption.
pub(crate) fn normalize(fault: &Fault) -> Diagnostic {
    match fault {
        Fault::Runtime(error) => Diagnostic {
            kind: error.kind,
            message: format!("{}{}", error.message, format_location(error.loc)),
            location: error.loc.map(|loc| Location {
                line: loc.line,
                column: loc.column,
            }),
            suggestions: error.suggestions.clone(),
        },
        Fault::ToolRuntime {
            kind,
            message,
            suggestions,
        } => Diagnostic {
            kind: *kind,
            message: message.clone(),
            location: None,
            suggestions: (!suggestions.is_empty()).then(|| suggestions.clone()),
        },
        Fault::Tool(message) => {
            Diagnostic::new(DiagnosticKind::ToolFailure, public_message(message))
        }
        Fault::Throw(value) => {
            let value = &value.0;
            let message = if has_runtime_reference(value) {
                "a non-data value".to_string()
            } else {
                match value {
                    Value::Str(text) => text.to_string(),
                    Value::Object(object) => match lock(object).get("message") {
                        Some(Value::Str(text)) => text.to_string(),
                        _ => stringify_thrown(value),
                    },
                    _ => stringify_thrown(value),
                }
            };
            Diagnostic::new(
                DiagnosticKind::ExecutionFailure,
                format!("Uncaught: {message}"),
            )
        }
        Fault::Interrupted => Diagnostic::new(
            DiagnosticKind::ExecutionFailure,
            "Execution was interrupted.",
        ),
    }
}

fn stringify_thrown(value: &Value) -> String {
    match convert::to_json(value, "Thrown value", false) {
        Ok(Some(json)) => crate::tools::render::json_stringify(&json),
        _ => convert::to_string(value),
    }
}

/// The value a `catch` binding (or an allSettled rejection reason) observes.
pub(crate) fn caught_value(fault: &Fault) -> Value {
    match fault {
        Fault::Throw(value) => value.0.clone(),
        Fault::Runtime(error) => {
            Value::object(ObjectData::error(error.error_name, error.message.clone()))
        }
        other => Value::object(ObjectData::error("Error", normalize(other).message)),
    }
}
