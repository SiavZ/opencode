//! Purpose: `Object.*` statics over data objects (keys/values/entries/hasOwn/assign/fromEntries).
//! Scope: stdlib/object.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: sandbox values read as empty objects; blocked keys are never written.
//! Contract tests: codemode/tests/stdlib.rs, codemode/tests/enumeration.rs

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::convert::{bounded, is_blocked_member, to_string};
use crate::value::{ArrayData, ObjectData, Value, lock};

const STATICS: &[&str] = &[
    "keys",
    "values",
    "entries",
    "hasOwn",
    "assign",
    "fromEntries",
];

/// `Object.keys(array)`: index strings plus own non-index properties.
pub(crate) fn array_keys(array: &ArrayData) -> Vec<String> {
    (0..array.items.len())
        .map(|index| index.to_string())
        .chain(array.extra.keys().cloned())
        .collect()
}

pub(crate) fn invoke(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    if !STATICS.contains(&name) {
        return Err(fail(
            format!("Object.{name} is not available in CodeMode."),
            loc,
        ));
    }
    let first = args.first().cloned().unwrap_or(Value::Undefined);
    let require_object = || -> Eval<Vec<(String, Value)>> {
        let value = bounded(&first, &format!("Object.{name} input"))?;
        match &value {
            value if value.is_sandbox() => Ok(Vec::new()),
            Value::Object(object) => Ok(lock(object).entries()),
            _ => Err(fail(format!("Object.{name} expects a data object."), loc)),
        }
    };
    let guarded = |out: &mut ObjectData, key: String, item: Value| -> Eval<()> {
        if is_blocked_member(&key) {
            return Err(fail(
                format!("Property '{key}' is not available in CodeMode."),
                loc,
            ));
        }
        out.set(key, item);
        Ok(())
    };
    Ok(match name {
        "keys" => {
            let value = bounded(&first, "Object.keys input")?;
            let keys = match &value {
                value if value.is_sandbox() => Vec::new(),
                Value::Array(array) => array_keys(&lock(array)),
                Value::Object(object) => lock(object).keys(),
                _ => return Err(fail("Object.keys expects a data object or array.", loc)),
            };
            Value::array(keys.into_iter().map(Value::str).collect())
        }
        "values" => Value::array(
            require_object()?
                .into_iter()
                .map(|(_, item)| item)
                .collect(),
        ),
        "entries" => Value::array(
            require_object()?
                .into_iter()
                .map(|(key, item)| Value::array(vec![Value::str(key), item]))
                .collect(),
        ),
        "hasOwn" => {
            let key = to_string(args.get(1).unwrap_or(&Value::Undefined));
            Value::Bool(require_object()?.iter().any(|(name, _)| *name == key))
        }
        "assign" => {
            let mut out = ObjectData::new();
            for source in args {
                if source.is_nullish() {
                    continue;
                }
                let value = bounded(source, "Object.assign input")?;
                match &value {
                    value if value.is_sandbox() => {}
                    Value::Object(object) => {
                        for (key, item) in lock(object).entries() {
                            guarded(&mut out, key, item)?;
                        }
                    }
                    _ => return Err(fail("Object.assign expects data objects.", loc)),
                }
            }
            Value::object(out)
        }
        _ => {
            let mut out = ObjectData::new();
            match &first {
                Value::Map(map) => {
                    for (key, item) in lock(map).entries.values().cloned().collect::<Vec<_>>() {
                        guarded(&mut out, to_string(&key), item)?;
                    }
                }
                Value::Params(params) => {
                    for (key, item) in lock(params).pairs.clone() {
                        guarded(&mut out, key, Value::str(item))?;
                    }
                }
                _ => {
                    let pairs = bounded(&first, "Object.fromEntries input")?;
                    let Value::Array(pairs) = pairs else {
                        return Err(fail(
                            "Object.fromEntries expects an array of [key, value] pairs.",
                            loc,
                        ));
                    };
                    for pair in lock(&pairs).items.clone() {
                        let Value::Array(pair) = pair else {
                            return Err(fail(
                                "Object.fromEntries expects [key, value] pairs.",
                                loc,
                            ));
                        };
                        let pair = lock(&pair).items.clone();
                        let key = to_string(pair.first().unwrap_or(&Value::Undefined));
                        guarded(
                            &mut out,
                            key,
                            pair.get(1).cloned().unwrap_or(Value::Undefined),
                        )?;
                    }
                }
            }
            Value::object(out)
        }
    })
}
