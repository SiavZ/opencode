//! Purpose: `Map` and `Set` construction and callback-free methods.
//! Scope: runtime.ts constructMap/constructSet/invokeMapMethod/invokeSetMethod (non-forEach) and
//! stdlib/collections.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: keys use SameValueZero (objects by identity); insertion order is preserved.
//! Contract tests: codemode/tests/stdlib.rs

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::{MapData, SetData, Shared, Value, lock, shared};

pub(crate) fn construct_map(init: Option<&Value>, loc: Loc) -> Eval<Value> {
    let mut data = MapData::default();
    let entries: Vec<Value> = match init {
        None | Some(Value::Undefined | Value::Null) => Vec::new(),
        Some(Value::Array(array)) => lock(array).items.clone(),
        Some(Value::Map(map)) => lock(map)
            .entries
            .values()
            .map(|(key, item)| Value::array(vec![key.clone(), item.clone()]))
            .collect(),
        Some(_) => {
            return Err(fail(
                "new Map(...) expects an array of [key, value] pairs, a Map, or no argument.",
                loc,
            ));
        }
    };
    for pair in entries {
        let Value::Array(pair) = pair else {
            return Err(fail("new Map(...) expects [key, value] pairs.", loc));
        };
        let pair = lock(&pair).items.clone();
        let key = pair.first().cloned().unwrap_or(Value::Undefined);
        let item = pair.get(1).cloned().unwrap_or(Value::Undefined);
        data.entries.insert(key.map_key(), (key, item));
    }
    Ok(Value::Map(shared(data)))
}

pub(crate) fn construct_set(init: Option<&Value>, loc: Loc) -> Eval<Value> {
    let mut data = SetData::default();
    let items: Vec<Value> = match init {
        None | Some(Value::Undefined | Value::Null) => Vec::new(),
        Some(Value::Array(array)) => lock(array).items.clone(),
        Some(Value::Set(set)) => lock(set).entries.values().cloned().collect(),
        Some(Value::Str(text)) => text.chars().map(|ch| Value::str(ch.to_string())).collect(),
        Some(_) => {
            return Err(fail(
                "new Set(...) expects an array, Set, string, or no argument.",
                loc,
            ));
        }
    };
    for item in items {
        data.entries.entry(item.map_key()).or_insert(item);
    }
    Ok(Value::Set(shared(data)))
}

fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Undefined)
}

pub(crate) fn map_method(
    map: &Shared<MapData>,
    name: &str,
    args: &[Value],
    loc: Loc,
) -> Eval<Value> {
    let mut data = lock(map);
    Ok(match name {
        "get" => data
            .entries
            .get(&arg(args, 0).map_key())
            .map_or(Value::Undefined, |(_, item)| item.clone()),
        "has" => Value::Bool(data.entries.contains_key(&arg(args, 0).map_key())),
        "set" => {
            let key = arg(args, 0);
            match data.entries.get_mut(&key.map_key()) {
                Some(entry) => entry.1 = arg(args, 1),
                None => {
                    data.entries.insert(key.map_key(), (key, arg(args, 1)));
                }
            }
            Value::Map(map.clone())
        }
        "delete" => Value::Bool(data.entries.shift_remove(&arg(args, 0).map_key()).is_some()),
        "clear" => {
            data.entries.clear();
            Value::Undefined
        }
        "keys" => Value::array(data.entries.values().map(|(key, _)| key.clone()).collect()),
        "values" => Value::array(
            data.entries
                .values()
                .map(|(_, item)| item.clone())
                .collect(),
        ),
        "entries" => Value::array(
            data.entries
                .values()
                .map(|(key, item)| Value::array(vec![key.clone(), item.clone()]))
                .collect(),
        ),
        _ => {
            return Err(fail(
                format!("Map method '{name}' is not available in CodeMode."),
                loc,
            ));
        }
    })
}

pub(crate) fn set_method(
    set: &Shared<SetData>,
    name: &str,
    args: &[Value],
    loc: Loc,
) -> Eval<Value> {
    let mut data = lock(set);
    Ok(match name {
        "has" => Value::Bool(data.entries.contains_key(&arg(args, 0).map_key())),
        "add" => {
            let item = arg(args, 0);
            data.entries.entry(item.map_key()).or_insert(item);
            Value::Set(set.clone())
        }
        "delete" => Value::Bool(data.entries.shift_remove(&arg(args, 0).map_key()).is_some()),
        "clear" => {
            data.entries.clear();
            Value::Undefined
        }
        "keys" | "values" => Value::array(data.entries.values().cloned().collect()),
        "entries" => Value::array(
            data.entries
                .values()
                .map(|item| Value::array(vec![item.clone(), item.clone()]))
                .collect(),
        ),
        _ => {
            return Err(fail(
                format!("Set method '{name}' is not available in CodeMode."),
                loc,
            ));
        }
    })
}
