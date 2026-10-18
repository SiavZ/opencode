//! Purpose: captured console output (log/info/debug/warn/error/dir/table) with total, debugger-
//! style formatting.
//! Scope: `runtime.ts` invokeConsole/formatConsole* (lines 1960-2075) and stdlib/console.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/call.rs
//! Invariants: formatting never fails the program; cycles render `[Circular]`, opaque references
//! render `[CodeMode reference]`; host paths are redacted.
//! Contract tests: codemode/tests/codemode.rs

use super::Interp;
use super::diagnose::{has_runtime_reference, public_message};
use super::ops::contains_opaque;
use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::convert::{bounded, to_json, to_string};
use crate::value::text::quote_json;
use crate::value::{Value, lock, number};

const CONSOLE_METHODS: [&str; 7] = ["log", "info", "debug", "warn", "error", "dir", "table"];
const MAX_CONSOLE_DEPTH: usize = 32;

impl Interp {
    pub(super) fn console(&mut self, name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
        if !CONSOLE_METHODS.contains(&name) {
            return Err(fail(
                format!("console.{name} is not available in CodeMode."),
                loc,
            ));
        }
        let message = match name {
            "dir" => args
                .first()
                .map_or_else(|| "undefined".into(), format_argument),
            "table" => format_table(args.first(), args.get(1))?,
            _ => {
                let prefix = match name {
                    "warn" => "[warn] ",
                    "error" => "[error] ",
                    "debug" => "[debug] ",
                    _ => "",
                };
                format!(
                    "{prefix}{}",
                    args.iter()
                        .map(format_argument)
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
        };
        self.logs.push(public_message(&message));
        Ok(Value::Undefined)
    }
}

fn format_argument(value: &Value) -> String {
    match value {
        Value::Undefined => "undefined".into(),
        Value::Str(text) => text.to_string(),
        _ => format_value(value, &mut Vec::new(), 0),
    }
}

fn format_value(value: &Value, seen: &mut Vec<usize>, depth: usize) -> String {
    match value {
        Value::Null | Value::Undefined | Value::ShortCircuit => return "null".into(),
        Value::Str(text) => return quote_json(text),
        Value::Number(value) => return number::format(*value),
        Value::Bool(flag) => return flag.to_string(),
        Value::Promise(_) => return "[Promise (await it to get its value)]".into(),
        Value::Date(_) | Value::RegExp(_) | Value::Url(_) | Value::Params(_) => {
            return to_string(value);
        }
        _ => {}
    }
    if depth > MAX_CONSOLE_DEPTH {
        return "...".into();
    }
    let id = value.identity().unwrap_or_default();
    if seen.contains(&id) {
        return "[Circular]".into();
    }
    if value.is_opaque_reference() {
        return "[CodeMode reference]".into();
    }
    seen.push(id);
    let rendered = match value {
        Value::Map(map) => {
            let entries: Vec<Value> = lock(map)
                .entries
                .values()
                .map(|(key, item)| Value::array(vec![key.clone(), item.clone()]))
                .collect();
            format!(
                "Map({}) {}",
                entries.len(),
                format_value(&Value::array(entries), seen, depth + 1)
            )
        }
        Value::Set(set) => {
            let items: Vec<Value> = lock(set).entries.values().cloned().collect();
            format!(
                "Set({}) {}",
                items.len(),
                format_value(&Value::array(items), seen, depth + 1)
            )
        }
        Value::Array(array) => {
            let items = lock(array).items.clone();
            format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| format_value(item, seen, depth + 1))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Object(object) => {
            let entries = lock(object).entries();
            format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, item)| format!(
                        "{}:{}",
                        quote_json(key),
                        format_value(item, seen, depth + 1)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => "[CodeMode reference]".into(),
    };
    seen.pop();
    rendered
}

fn format_table(value: Option<&Value>, columns: Option<&Value>) -> Eval<String> {
    let Some(value) = value.filter(|value| !matches!(value, Value::Undefined)) else {
        return Ok("undefined".into());
    };
    if contains_opaque(value) {
        return Ok("[CodeMode reference]".into());
    }
    let data = bounded(value, "console.table argument")?;
    let columns: Option<Vec<String>> = match columns {
        None | Some(Value::Undefined) => None,
        Some(value) if has_runtime_reference(value) => None,
        Some(value) => match to_json(value, "console.table columns", true)? {
            Some(serde_json::Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| match item {
                        serde_json::Value::String(text) => text.clone(),
                        other => crate::tools::render::json_stringify(other),
                    })
                    .collect(),
            ),
            _ => None,
        },
    };
    let row_values = |item: &Value| -> Vec<(String, Value)> {
        match item {
            Value::Object(object) => {
                let object = lock(object);
                match &columns {
                    Some(columns) => columns
                        .iter()
                        .map(|column| {
                            (
                                column.clone(),
                                object.get(column).cloned().unwrap_or(Value::Undefined),
                            )
                        })
                        .collect(),
                    None => object.entries(),
                }
            }
            other => vec![("Value".into(), other.clone())],
        }
    };
    let rows: Vec<(String, Vec<(String, Value)>)> = match &data {
        Value::Array(array) => lock(array)
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), row_values(item)))
            .collect(),
        Value::Object(object) => lock(object)
            .entries()
            .into_iter()
            .map(|(key, item)| (key, row_values(&item)))
            .collect(),
        other => vec![("0".into(), vec![("Value".into(), other.clone())])],
    };
    let keys: Vec<String> = match &columns {
        Some(columns) => columns.clone(),
        None => {
            let mut keys: Vec<String> = Vec::new();
            for (_, values) in &rows {
                for (key, _) in values {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
            }
            keys
        }
    };
    let mut lines = vec![
        std::iter::once("(index)".to_string())
            .chain(keys.iter().cloned())
            .collect::<Vec<_>>()
            .join("\t"),
    ];
    for (index, values) in rows {
        let cells = keys.iter().map(|key| {
            match values
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
            {
                None | Some(Value::Undefined) => String::new(),
                Some(Value::Str(text)) => text.to_string(),
                Some(value) => format_value(value, &mut Vec::new(), 0),
            }
        });
        lines.push(
            std::iter::once(index)
                .chain(cells)
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    Ok(lines.join("\n"))
}
