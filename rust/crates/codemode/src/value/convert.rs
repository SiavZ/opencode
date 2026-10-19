//! Purpose: the plain-data contract (`copyIn`/`copyOut`), JSON bridging, and JS coercions.
//! Scope: depth/cycle/blocked-key/data-only validation shared by every data checkpoint.
//! Uses:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/stdlib/date.rs
//! |-- codemode/src/stdlib/url.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/number.rs
//! External:
//! |-- date::to_iso
//! |-- f64::NAN
//! |-- f64::from
//! |-- number::format
//! |-- number::parse_number
//! |-- serde_json::Map::new
//! |-- serde_json::Number::from_f64
//! |-- serde_json::Value
//! `-- u8::from
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/interp/patterns.rs
//! |-- codemode/src/lib.rs
//! |-- codemode/src/openapi/naming.rs
//! |-- codemode/src/openapi/spec.rs
//! |-- codemode/src/run.rs
//! |-- codemode/src/stdlib/array.rs
//! |-- codemode/src/stdlib/date.rs
//! |-- codemode/src/stdlib/json.rs
//! |-- codemode/src/stdlib/number.rs
//! |-- codemode/src/stdlib/object.rs
//! |-- codemode/src/stdlib/regexp.rs
//! |-- codemode/src/stdlib/string.rs
//! |-- codemode/src/stdlib/url.rs
//! |-- codemode/src/stdlib/value.rs
//! |-- codemode/src/tools/runtime.rs
//! |-- codemode/src/tools/schema.rs
//! `-- codemode/src/value/mod.rs
//! Invariants: boundary mode never emits sandbox internals (Date/URL -> strings, other
//! stdlib values -> {}); un-awaited promises are always rejected with an await hint.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs, codemode/tests/promise.rs

use super::{ArrayData, ObjectData, Value, lock, number, shared};
use crate::error::{Eval, data_error};
use crate::stdlib::date;
use serde_json::Value as Json;
use std::collections::HashSet;

/// Fixed nesting ceiling for values crossing a data checkpoint.
pub(crate) const MAX_VALUE_DEPTH: usize = 32;

const BLOCKED: [&str; 3] = ["__proto__", "constructor", "prototype"];

pub(crate) fn is_blocked_member(name: &str) -> bool {
    BLOCKED.contains(&name)
}

/// Intra-sandbox checkpoint (`boundedData`): copies containers, keeps stdlib values as leaves.
pub(crate) fn bounded(value: &Value, label: &str) -> Eval<Value> {
    copy_in(value, label, 0, &mut HashSet::new())
}

fn copy_in(value: &Value, label: &str, depth: usize, seen: &mut HashSet<usize>) -> Eval<Value> {
    if depth > MAX_VALUE_DEPTH {
        return Err(data_error(format!(
            "{label} exceeds the maximum value depth of {MAX_VALUE_DEPTH}."
        )));
    }
    match value {
        Value::Undefined | Value::ShortCircuit => Ok(Value::Undefined),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Str(_) => Ok(value.clone()),
        Value::Promise(_) => Err(unawaited(label)),
        value if value.is_sandbox() => Ok(value.clone()),
        Value::Array(array) => {
            let id = value.identity().unwrap_or_default();
            if !seen.insert(id) {
                return Err(data_error(format!("{label} contains a circular value.")));
            }
            let items = lock(array).items.clone();
            let copied = items
                .iter()
                .map(|item| copy_in(item, label, depth + 1, seen))
                .collect::<Eval<Vec<_>>>()?;
            seen.remove(&id);
            Ok(Value::array(copied))
        }
        Value::Object(object) => {
            let id = value.identity().unwrap_or_default();
            if !seen.insert(id) {
                return Err(data_error(format!("{label} contains a circular value.")));
            }
            let entries = lock(object).entries();
            let mut copied = ObjectData::new();
            for (key, item) in entries {
                if is_blocked_member(&key) {
                    return Err(data_error(format!(
                        "{label} contains blocked property '{key}'."
                    )));
                }
                copied.set(key, copy_in(&item, label, depth + 1, seen)?);
            }
            seen.remove(&id);
            Ok(Value::object(copied))
        }
        _ => Err(data_error(format!(
            "{label} must contain plain objects only."
        ))),
    }
}

fn unawaited(label: &str) -> crate::error::Fault {
    data_error(format!(
        "{label} contains an un-awaited Promise; await tool calls (e.g. `const result = await tools.ns.tool(...)`) before using their results."
    ))
}

/// Host boundary (`copyOut(copyIn(value))`): validates and serializes to JSON. `None` means a
/// top-level `undefined` when `undefined_as_null` is false.
pub(crate) fn to_json(value: &Value, label: &str, undefined_as_null: bool) -> Eval<Option<Json>> {
    to_json_at(value, label, 0, &mut HashSet::new(), undefined_as_null)
}

fn to_json_at(
    value: &Value,
    label: &str,
    depth: usize,
    seen: &mut HashSet<usize>,
    undefined_as_null: bool,
) -> Eval<Option<Json>> {
    if depth > MAX_VALUE_DEPTH {
        return Err(data_error(format!(
            "{label} exceeds the maximum value depth of {MAX_VALUE_DEPTH}."
        )));
    }
    Ok(Some(match value {
        Value::Undefined | Value::ShortCircuit => {
            return Ok(undefined_as_null.then_some(Json::Null));
        }
        Value::Null => Json::Null,
        Value::Bool(flag) => Json::Bool(*flag),
        Value::Number(number) => json_number(*number),
        Value::Str(text) => Json::String(text.to_string()),
        Value::Promise(_) => return Err(unawaited(label)),
        Value::Date(time) => date::to_iso(**time).map_or(Json::Null, Json::String),
        Value::Url(url) => Json::String(lock(url).url.as_str().to_string()),
        Value::RegExp(_) | Value::Map(_) | Value::Set(_) | Value::Params(_) => {
            Json::Object(Default::default())
        }
        Value::Array(array) => {
            let id = value.identity().unwrap_or_default();
            if !seen.insert(id) {
                return Err(data_error(format!("{label} contains a circular value.")));
            }
            let items = lock(array).items.clone();
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(
                    to_json_at(item, label, depth + 1, seen, undefined_as_null)?
                        .unwrap_or(Json::Null),
                );
            }
            seen.remove(&id);
            Json::Array(out)
        }
        Value::Object(object) => {
            let id = value.identity().unwrap_or_default();
            if !seen.insert(id) {
                return Err(data_error(format!("{label} contains a circular value.")));
            }
            let entries = lock(object).entries();
            let mut out = serde_json::Map::new();
            for (key, item) in entries {
                if is_blocked_member(&key) {
                    return Err(data_error(format!(
                        "{label} contains blocked property '{key}'."
                    )));
                }
                if let Some(json) = to_json_at(&item, label, depth + 1, seen, undefined_as_null)? {
                    out.insert(key, json);
                }
            }
            seen.remove(&id);
            Json::Object(out)
        }
        _ => {
            return Err(data_error(format!(
                "{label} must contain plain objects only."
            )));
        }
    }))
}

/// A JS number as JSON: integral values print without a fraction, non-finite become null.
pub(crate) fn json_number(number: f64) -> Json {
    if !number.is_finite() {
        return Json::Null;
    }
    if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
        return Json::from(number as i64);
    }
    serde_json::Number::from_f64(number).map_or(Json::Null, Json::Number)
}

/// Host JSON into the sandbox (`copyIn(hostValue, label)`), enforcing depth and blocked keys.
pub(crate) fn from_json(json: &Json, label: &str) -> Eval<Value> {
    from_json_at(json, label, 0)
}

fn from_json_at(json: &Json, label: &str, depth: usize) -> Eval<Value> {
    if depth > MAX_VALUE_DEPTH {
        return Err(data_error(format!(
            "{label} exceeds the maximum value depth of {MAX_VALUE_DEPTH}."
        )));
    }
    Ok(match json {
        Json::Null => Value::Null,
        Json::Bool(flag) => Value::Bool(*flag),
        Json::Number(number) => Value::Number(number.as_f64().unwrap_or(f64::NAN)),
        Json::String(text) => Value::str(text),
        Json::Array(items) => Value::Array(shared(ArrayData::new(
            items
                .iter()
                .map(|item| from_json_at(item, label, depth + 1))
                .collect::<Eval<Vec<_>>>()?,
        ))),
        Json::Object(map) => {
            let mut data = ObjectData::new();
            for (key, item) in map {
                if is_blocked_member(key) {
                    return Err(data_error(format!(
                        "{label} contains blocked property '{key}'."
                    )));
                }
                data.set(key.clone(), from_json_at(item, label, depth + 1)?);
            }
            Value::object(data)
        }
    })
}

/// `coerceToString`: JS `String(value)` for data and sandbox values.
pub(crate) fn to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Undefined | Value::ShortCircuit => "undefined".into(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number::format(*number),
        Value::Str(text) => text.to_string(),
        Value::Date(time) => date::to_iso(**time).unwrap_or_else(|| "Invalid Date".into()),
        Value::RegExp(regex) => format!("/{}/{}", regex.source, regex.flags),
        Value::Map(_) => "[object Map]".into(),
        Value::Set(_) => "[object Set]".into(),
        Value::Url(url) => lock(url).url.as_str().to_string(),
        Value::Params(params) => crate::stdlib::url::serialize_params(&lock(params).pairs),
        Value::Array(array) => {
            let items = lock(array).items.clone();
            items
                .iter()
                .map(|item| {
                    if item.is_nullish() {
                        String::new()
                    } else {
                        to_string(item)
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        }
        Value::Object(_) | Value::Promise(_) => "[object Object]".into(),
        _ => "function () { [native code] }".into(),
    }
}

/// `coerceToNumber`: JS `Number(value)` with null-prototype objects as NaN.
pub(crate) fn to_number(value: &Value) -> f64 {
    match value {
        Value::Date(time) => **time,
        Value::Undefined | Value::ShortCircuit => f64::NAN,
        Value::Null => 0.0,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Number(number) => *number,
        Value::Str(text) => number::parse_number(text),
        Value::Array(_) => number::parse_number(&to_string(value)),
        _ => f64::NAN,
    }
}
