//! Purpose: `JSON.stringify` (with indentation) and `JSON.parse` over the data boundary.
//! Scope: stdlib/json.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/tools/render.rs
//! `-- codemode/src/value/convert.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: stringify validates like the host boundary (promises and cycles are rejected);
//! parse never yields blocked keys.
//! Contract tests: codemode/tests/stdlib.rs, codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{Eval, Fault, fail, fail_named, unsupported_feature};
use crate::tools::render::{entries, json_stringify};
use crate::value::Value;
use crate::value::convert::{from_json, to_json};
use crate::value::text::quote_json;
use serde_json::Value as Json;

pub(crate) fn invoke(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    match name {
        "stringify" => {
            if matches!(args.get(1), Some(Value::Array(_) | Value::Function(_))) {
                return Err(unsupported_feature(
                    "JSON.stringify replacers are not supported in CodeMode.",
                    loc,
                ));
            }
            let indent = match args.get(2) {
                Some(Value::Number(count)) => " ".repeat(count.clamp(0.0, 10.0) as usize),
                Some(Value::Str(text)) => text.chars().take(10).collect(),
                _ => String::new(),
            };
            let value = args.first().cloned().unwrap_or(Value::Undefined);
            Ok(match to_json(&value, "JSON.stringify value", false)? {
                None => Value::Undefined,
                Some(json) if indent.is_empty() => Value::str(json_stringify(&json)),
                Some(json) => Value::str(pretty(&json, &indent, "")),
            })
        }
        "parse" => {
            let Some(Value::Str(text)) = args.first() else {
                return Err(fail("JSON.parse expects a string.", loc));
            };
            let parsed = serde_json::from_str::<Json>(text)
                .map_err(|error| error.to_string())
                .and_then(|json| {
                    from_json(&json, "JSON.parse result").map_err(|fault| fault_message(&fault))
                });
            parsed.map_err(|reason| {
                fail_named(
                    format!("JSON.parse received invalid JSON: {reason}"),
                    loc,
                    "SyntaxError",
                )
            })
        }
        _ => Err(fail(
            format!("JSON.{name} is not available in CodeMode."),
            loc,
        )),
    }
}

fn fault_message(fault: &Fault) -> String {
    match fault {
        Fault::ToolRuntime { message, .. } => message.clone(),
        _ => "invalid value".into(),
    }
}

fn pretty(json: &Json, indent: &str, current: &str) -> String {
    let inner = format!("{current}{indent}");
    match json {
        Json::Array(items) if !items.is_empty() => format!(
            "[\n{}\n{current}]",
            items
                .iter()
                .map(|item| format!("{inner}{}", pretty(item, indent, &inner)))
                .collect::<Vec<_>>()
                .join(",\n")
        ),
        Json::Object(map) if !map.is_empty() => format!(
            "{{\n{}\n{current}}}",
            entries(map)
                .into_iter()
                .map(|(key, item)| format!(
                    "{inner}{}: {}",
                    quote_json(key),
                    pretty(item, indent, &inner)
                ))
                .collect::<Vec<_>>()
                .join(",\n")
        ),
        other => json_stringify(other),
    }
}
