//! Purpose: `Array` statics and the callback-free array methods.
//! Scope: runtime.ts invokeArrayStatic (lines 520-555) and the non-callback half of
//! invokeArrayMethod (lines 2560-2660).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/lib.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/text.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: mutators act in place; non-mutating methods return fresh arrays.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

use crate::ast::Loc;
use crate::error::{Eval, fail, unsupported_feature};
use crate::value::convert::{bounded, to_string};
use crate::value::text::{code_points, relative_index};
use crate::value::{ArrayData, Shared, Value, lock};

pub(crate) fn invoke_static(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    match name {
        "isArray" => Ok(Value::Bool(matches!(args.first(), Some(Value::Array(_))))),
        "of" => Ok(Value::array(args.to_vec())),
        "from" => {
            if args.len() > 1 {
                return Err(unsupported_feature(
                    "Array.from(...) does not support a map function in CodeMode; call .map() on the result instead.",
                    loc,
                ));
            }
            let first = args.first().cloned().unwrap_or(Value::Undefined);
            if matches!(first, Value::Map(_) | Value::Set(_) | Value::Params(_)) {
                return Ok(Value::array(
                    crate::interp_spread(&first).unwrap_or_default(),
                ));
            }
            match bounded(&first, "Array.from input")? {
                Value::Str(text) => Ok(Value::array(
                    code_points(&text).into_iter().map(Value::str).collect(),
                )),
                Value::Array(array) => Ok(Value::array(lock(&array).items.clone())),
                Value::Object(object) => {
                    let object = lock(&object);
                    match object.get("length") {
                        Some(Value::Number(length)) => {
                            let length = if length.is_finite() && *length > 0.0 {
                                *length as usize
                            } else {
                                0
                            };
                            Ok(Value::array(
                                (0..length)
                                    .map(|index| {
                                        object
                                            .get(&index.to_string())
                                            .cloned()
                                            .unwrap_or(Value::Undefined)
                                    })
                                    .collect(),
                            ))
                        }
                        _ => Err(fail(
                            "Array.from expects an array, string, Map, Set, or array-like value.",
                            loc,
                        )),
                    }
                }
                _ => Err(fail(
                    "Array.from expects an array, string, Map, Set, or array-like value.",
                    loc,
                )),
            }
        }
        _ => Err(fail(
            format!("Array.{name} is not available in CodeMode."),
            loc,
        )),
    }
}

fn opt_number(
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    loc: Loc,
) -> Eval<Option<f64>> {
    match args.get(index) {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Number(number)) => Ok(Some(*number)),
        Some(_) => Err(fail(
            format!("Array.{name} expects {label} to be a number."),
            loc,
        )),
    }
}

fn flatten(items: &[Value], depth: f64, out: &mut Vec<Value>) {
    for item in items {
        match item {
            Value::Array(inner) if depth >= 1.0 => {
                flatten(&lock(inner).items.clone(), depth - 1.0, out)
            }
            other => out.push(other.clone()),
        }
    }
}

/// Non-callback array methods; `None` means the method takes a callback.
pub(crate) fn plain_method(
    array: &Shared<ArrayData>,
    name: &str,
    args: &[Value],
    loc: Loc,
) -> Eval<Option<Value>> {
    let num = |index: usize, label: &str| opt_number(args, index, name, label, loc);
    let items = lock(array).items.clone();
    let len = items.len();
    Ok(Some(match name {
        "join" => {
            if args.len() > 1 || (args.len() == 1 && !matches!(args[0], Value::Str(_))) {
                return Err(fail(
                    "Array.join expects zero arguments or one string separator.",
                    loc,
                ));
            }
            let separator = match args.first() {
                Some(Value::Str(text)) => text.to_string(),
                _ => ",".into(),
            };
            let Value::Array(copy) = bounded(&Value::Array(array.clone()), "Array.join input")?
            else {
                return Ok(None);
            };
            let copied = lock(&copy).items.clone();
            Value::str(
                copied
                    .iter()
                    .map(|item| {
                        if item.is_nullish() {
                            String::new()
                        } else {
                            to_string(item)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(&separator),
            )
        }
        "includes" => {
            if args.is_empty() || args.len() > 2 {
                return Err(fail(
                    "Array.includes expects a value and optional start index.",
                    loc,
                ));
            }
            let start = num(1, "start index")?.map_or(0, |start| relative_index(start, len));
            Value::Bool(
                items[start..]
                    .iter()
                    .any(|item| item.same_value_zero(&args[0])),
            )
        }
        "indexOf" => {
            let start = num(1, "start index")?.map_or(0, |start| relative_index(start, len));
            let target = args.first().cloned().unwrap_or(Value::Undefined);
            Value::Number(
                items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .find(|(_, item)| item.strict_equals(&target))
                    .map_or(-1.0, |(index, _)| index as f64),
            )
        }
        "lastIndexOf" => {
            let target = args.first().cloned().unwrap_or(Value::Undefined);
            let from = match num(1, "start index")? {
                None => len as f64 - 1.0,
                Some(from) if from < 0.0 => len as f64 + from.trunc(),
                Some(from) => from.trunc().min(len as f64 - 1.0),
            };
            Value::Number(if from < 0.0 {
                -1.0
            } else {
                (0..=from as usize)
                    .rev()
                    .find(|index| items[*index].strict_equals(&target))
                    .map_or(-1.0, |index| index as f64)
            })
        }
        "at" => {
            let index = num(0, "index")?.unwrap_or(0.0).trunc();
            let resolved = if index < 0.0 {
                len as f64 + index
            } else {
                index
            };
            if resolved < 0.0 || resolved >= len as f64 {
                Value::Undefined
            } else {
                items[resolved as usize].clone()
            }
        }
        "slice" => {
            let start = relative_index(num(0, "start")?.unwrap_or(0.0), len);
            let end = num(1, "end")?.map_or(len, |end| relative_index(end, len));
            Value::array(if start < end {
                items[start..end].to_vec()
            } else {
                Vec::new()
            })
        }
        "concat" => {
            let mut out = items;
            for arg in args {
                match arg {
                    Value::Array(inner) => out.extend(lock(inner).items.clone()),
                    other => out.push(other.clone()),
                }
            }
            Value::array(out)
        }
        "flat" => {
            let mut out = Vec::new();
            flatten(&items, num(0, "depth")?.unwrap_or(1.0), &mut out);
            Value::array(out)
        }
        "reverse" | "toReversed" => Value::array(items.into_iter().rev().collect()),
        "with" => {
            let index = num(0, "index")?.unwrap_or(0.0).trunc();
            let resolved = if index < 0.0 {
                len as f64 + index
            } else {
                index
            };
            if resolved < 0.0 || resolved >= len as f64 {
                return Err(fail("Array.with index is out of range.", loc));
            }
            let mut copy = items;
            copy[resolved as usize] = args.get(1).cloned().unwrap_or(Value::Undefined);
            Value::array(copy)
        }
        "pop" => lock(array).items.pop().unwrap_or(Value::Undefined),
        "shift" => {
            let mut data = lock(array);
            if data.items.is_empty() {
                Value::Undefined
            } else {
                data.items.remove(0)
            }
        }
        "splice" => {
            let mut data = lock(array);
            if args.is_empty() {
                return Ok(Some(Value::array(Vec::new())));
            }
            let start = relative_index(num(0, "start")?.unwrap_or(0.0), len);
            let delete = if args.len() == 1 {
                len - start
            } else {
                let count = num(1, "delete count")?.unwrap_or(0.0);
                (if count.is_nan() { 0.0 } else { count.trunc() }).clamp(0.0, (len - start) as f64)
                    as usize
            };
            let removed: Vec<Value> = data
                .items
                .splice(start..start + delete, args.iter().skip(2).cloned())
                .collect();
            Value::array(removed)
        }
        "fill" => {
            let start = relative_index(num(1, "start")?.unwrap_or(0.0), len);
            let end = num(2, "end")?.map_or(len, |end| relative_index(end, len));
            let mut data = lock(array);
            for index in start..end.max(start) {
                data.items[index] = args.first().cloned().unwrap_or(Value::Undefined);
            }
            Value::Array(array.clone())
        }
        "copyWithin" => {
            let target = relative_index(num(0, "target index")?.unwrap_or(0.0), len);
            let start = relative_index(num(1, "start")?.unwrap_or(0.0), len);
            let end = num(2, "end")?.map_or(len, |end| relative_index(end, len));
            let count = end.saturating_sub(start).min(len - target);
            let mut data = lock(array);
            let source: Vec<Value> = data.items[start..start + count].to_vec();
            data.items[target..target + count].clone_from_slice(&source);
            Value::Array(array.clone())
        }
        "keys" => Value::array((0..len).map(|index| Value::Number(index as f64)).collect()),
        "values" => Value::array(items),
        "entries" => Value::array(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| Value::array(vec![Value::Number(index as f64), item]))
                .collect(),
        ),
        _ => return Ok(None),
    }))
}
