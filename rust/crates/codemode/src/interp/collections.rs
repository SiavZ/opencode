//! Purpose: callback-taking collection methods (arrays, Map, Set, URLSearchParams, string
//! replacers), spread/enumeration helpers, and comparator sorting.
//! Scope: `runtime.ts` invokeStringReplacer/applyCollectionCallback/invokeMapMethod/
//! invokeSetMethod/invokeURLSearchParamsMethod/invokeArrayMethod/sortArray (lines 2290-2800).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/stdlib/mod.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- stdlib::array::plain_method
//! |-- stdlib::collections::map_method
//! |-- stdlib::collections::set_method
//! |-- stdlib::object::array_keys
//! |-- stdlib::regexp::replacer_matches
//! |-- stdlib::url::invoke_uri
//! |-- stdlib::url::params_method
//! `-- stdlib::value::invoke_coercion
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/interp/promise.rs
//! `-- codemode/src/interp/stmt.rs
//! Invariants: callbacks iterate a snapshot; insertions that would create cycles are rejected
//! before mutation.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

use super::Interp;
use super::members::reject_circular_insertion;
use crate::ast::Loc;
use crate::error::{Eval, fail, fail_named};
use crate::stdlib;
use crate::value::convert::{bounded, to_number, to_string};
use crate::value::text::code_points;
use crate::value::{ArrayData, MapData, ParamsData, SetData, Shared, Value, lock};

/// Items produced by spread / for...of: arrays, strings (code points), Map entries, Set values,
/// URLSearchParams pairs.
pub(crate) fn spread_items(value: &Value) -> Option<Vec<Value>> {
    Some(match value {
        Value::Array(array) => lock(array).items.clone(),
        Value::Str(text) => code_points(text).into_iter().map(Value::str).collect(),
        Value::Map(map) => lock(map)
            .entries
            .values()
            .map(|(key, item)| Value::array(vec![key.clone(), item.clone()]))
            .collect(),
        Value::Set(set) => lock(set).entries.values().cloned().collect(),
        Value::Params(params) => lock(params)
            .pairs
            .iter()
            .map(|(key, item)| Value::array(vec![Value::str(key), Value::str(item)]))
            .collect(),
        _ => return None,
    })
}

impl Interp {
    /// Own enumerable keys shared by `for...in` and `Object.keys` over tool references.
    pub(super) fn enumerable_keys(&self, value: &Value) -> Eval<Option<Vec<String>>> {
        Ok(Some(match value {
            Value::Tool(path) => self.pool.tools().keys(path)?,
            Value::Array(array) => stdlib::object::array_keys(&lock(array)),
            Value::Object(object) => lock(object).keys(),
            _ => return Ok(None),
        }))
    }

    /// Runs a callback (user function or builtin callable).
    pub(super) async fn callback(
        &mut self,
        callback: &Value,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        match callback {
            Value::Coercion(coercion) => stdlib::value::invoke_coercion(*coercion, &args, loc),
            Value::Uri(function) => stdlib::url::invoke_uri(*function, &args, loc),
            Value::Function(function) => self.invoke(function, args).await,
            _ => Err(fail("Expected a function callback.", loc)),
        }
    }

    pub(super) async fn string_replacer(
        &mut self,
        value: &str,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        let callback = args[1].clone();
        let matches = stdlib::regexp::replacer_matches(value, name, &args[0], loc)?;
        let mut output = String::new();
        let mut end = 0;
        for found in matches {
            output.push_str(&value[end..found.start]);
            let replaced = self.callback(&callback, found.args, loc).await?;
            output.push_str(&to_string(&bounded(
                &replaced,
                &format!("String.{name} replacer result"),
            )?));
            end = found.end;
        }
        output.push_str(&value[end..]);
        bounded(&Value::str(output), &format!("String.{name} result"))
    }

    pub(super) async fn map_method(
        &mut self,
        map: Shared<MapData>,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        if name == "forEach" {
            let callback = require_callback(args.first(), "Map.forEach", loc)?;
            let entries: Vec<(Value, Value)> = lock(&map).entries.values().cloned().collect();
            for (key, item) in entries {
                self.callback(&callback, vec![item, key, Value::Map(map.clone())], loc)
                    .await?;
            }
            return Ok(Value::Undefined);
        }
        stdlib::collections::map_method(&map, name, &args, loc)
    }

    pub(super) async fn set_method(
        &mut self,
        set: Shared<SetData>,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        if name == "forEach" {
            let callback = require_callback(args.first(), "Set.forEach", loc)?;
            let items: Vec<Value> = lock(&set).entries.values().cloned().collect();
            for item in items {
                self.callback(
                    &callback,
                    vec![item.clone(), item, Value::Set(set.clone())],
                    loc,
                )
                .await?;
            }
            return Ok(Value::Undefined);
        }
        stdlib::collections::set_method(&set, name, &args, loc)
    }

    pub(super) async fn params_method(
        &mut self,
        params: Shared<ParamsData>,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        if name == "forEach" {
            if args.is_empty() {
                return Err(fail_named(
                    "URLSearchParams.forEach requires 1 argument.",
                    loc,
                    "TypeError",
                ));
            }
            let callback = require_callback(args.first(), "URLSearchParams.forEach", loc)?;
            let pairs = lock(&params).pairs.clone();
            for (key, item) in pairs {
                self.callback(
                    &callback,
                    vec![
                        Value::str(item),
                        Value::str(key),
                        Value::Params(params.clone()),
                    ],
                    loc,
                )
                .await?;
            }
            return Ok(Value::Undefined);
        }
        stdlib::url::params_method(&params, name, &args, loc)
    }

    pub(super) async fn array_method(
        &mut self,
        array: Shared<ArrayData>,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        let target = Value::Array(array.clone());
        match name {
            "push" | "unshift" => {
                for item in &args {
                    reject_circular_insertion(&target, item, &format!("Array.{name} result"), loc)?;
                }
                let mut data = lock(&array);
                if name == "push" {
                    data.items.extend(args);
                } else {
                    data.items.splice(0..0, args);
                }
                return Ok(Value::Number(data.items.len() as f64));
            }
            "splice" => {
                for item in args.iter().skip(2) {
                    reject_circular_insertion(&target, item, "Array.splice result", loc)?;
                }
            }
            "fill" => reject_circular_insertion(
                &target,
                args.first().unwrap_or(&Value::Undefined),
                "Array.fill result",
                loc,
            )?,
            "sort" | "toSorted" => return self.sort(&array, args.first(), loc).await,
            _ => {}
        }
        if let Some(result) = stdlib::array::plain_method(&array, name, &args, loc)? {
            return Ok(result);
        }
        let callback = require_callback(args.first(), &format!("Array.{name}"), loc)?;
        let items = lock(&array).items.clone();
        let snapshot = Value::array(items.clone());
        let index = |position: usize| Value::Number(position as f64);
        match name {
            "map" | "flatMap" | "filter" => {
                let mut values = Vec::new();
                for (position, item) in items.iter().enumerate() {
                    let result = self
                        .callback(
                            &callback,
                            vec![item.clone(), index(position), snapshot.clone()],
                            loc,
                        )
                        .await?;
                    match name {
                        "map" => values.push(result),
                        "filter" => {
                            if result.truthy() {
                                values.push(item.clone());
                            }
                        }
                        _ => match &result {
                            Value::Array(inner) => values.extend(lock(inner).items.clone()),
                            _ => values.push(result),
                        },
                    }
                }
                Ok(Value::array(values))
            }
            "find" | "findIndex" | "some" | "every" | "findLast" | "findLastIndex" => {
                let reverse = name.starts_with("findLast");
                let order: Vec<usize> = if reverse {
                    (0..items.len()).rev().collect()
                } else {
                    (0..items.len()).collect()
                };
                for position in order {
                    let item = items[position].clone();
                    let passed = self
                        .callback(
                            &callback,
                            vec![item.clone(), index(position), snapshot.clone()],
                            loc,
                        )
                        .await?
                        .truthy();
                    match name {
                        "find" | "findLast" if passed => return Ok(item),
                        "findIndex" | "findLastIndex" if passed => return Ok(index(position)),
                        "some" if passed => return Ok(Value::Bool(true)),
                        "every" if !passed => return Ok(Value::Bool(false)),
                        _ => {}
                    }
                }
                Ok(match name {
                    "find" | "findLast" => Value::Undefined,
                    "findIndex" | "findLastIndex" => Value::Number(-1.0),
                    "some" => Value::Bool(false),
                    _ => Value::Bool(true),
                })
            }
            "forEach" => {
                for (position, item) in items.iter().enumerate() {
                    self.callback(
                        &callback,
                        vec![item.clone(), index(position), snapshot.clone()],
                        loc,
                    )
                    .await?;
                }
                Ok(Value::Undefined)
            }
            "reduce" | "reduceRight" => {
                let order: Vec<usize> = if name == "reduce" {
                    (0..items.len()).collect()
                } else {
                    (0..items.len()).rev().collect()
                };
                let mut order = order.into_iter();
                let mut accumulator = if args.len() >= 2 {
                    args[1].clone()
                } else {
                    let Some(first) = order.next() else {
                        return Err(fail(
                            format!("Array.{name} of an empty array with no initial value."),
                            loc,
                        ));
                    };
                    items[first].clone()
                };
                for position in order {
                    accumulator = self
                        .callback(
                            &callback,
                            vec![
                                accumulator,
                                items[position].clone(),
                                index(position),
                                snapshot.clone(),
                            ],
                            loc,
                        )
                        .await?;
                }
                Ok(accumulator)
            }
            _ => Err(fail(
                format!("Array method '{name}' is not available in CodeMode."),
                loc,
            )),
        }
    }

    async fn sort(
        &mut self,
        array: &Shared<ArrayData>,
        comparator: Option<&Value>,
        loc: Loc,
    ) -> Eval<Value> {
        let items = lock(array).items.clone();
        let comparator = match comparator {
            None | Some(Value::Undefined) => None,
            Some(Value::Function(function)) => Some(function.clone()),
            Some(_) => {
                return Err(fail(
                    "Array.sort expects an arrow function comparator.",
                    loc,
                ));
            }
        };
        let Some(comparator) = comparator else {
            let mut sorted = items;
            sorted.sort_by(|left, right| {
                crate::value::text::compare16(&to_string(left), &to_string(right))
            });
            return Ok(Value::array(sorted));
        };
        let undefined_count = items
            .iter()
            .filter(|item| matches!(item, Value::Undefined))
            .count();
        let defined: Vec<Value> = items
            .into_iter()
            .filter(|item| !matches!(item, Value::Undefined))
            .collect();
        let mut sorted = self.merge_sort(defined, &comparator).await?;
        sorted.extend(std::iter::repeat_n(Value::Undefined, undefined_count));
        Ok(Value::array(sorted))
    }

    fn merge_sort<'a>(
        &'a mut self,
        items: Vec<Value>,
        comparator: &'a std::sync::Arc<crate::value::FunctionValue>,
    ) -> super::Fut<'a, Vec<Value>> {
        Box::pin(async move {
            if items.len() <= 1 {
                return Ok(items);
            }
            let mut left = items;
            let right_half = left.split_off(left.len() / 2);
            let left = self.merge_sort(left, comparator).await?;
            let right = self.merge_sort(right_half, comparator).await?;
            let mut merged = Vec::with_capacity(left.len() + right.len());
            let (mut left_index, mut right_index) = (0, 0);
            while left_index < left.len() && right_index < right.len() {
                let order = to_number(
                    &self
                        .invoke(
                            comparator,
                            vec![left[left_index].clone(), right[right_index].clone()],
                        )
                        .await?,
                );
                if order.is_nan() || order <= 0.0 {
                    merged.push(left[left_index].clone());
                    left_index += 1;
                } else {
                    merged.push(right[right_index].clone());
                    right_index += 1;
                }
            }
            merged.extend_from_slice(&left[left_index..]);
            merged.extend_from_slice(&right[right_index..]);
            Ok(merged)
        })
    }
}

fn require_callback(callback: Option<&Value>, name: &str, loc: Loc) -> Eval<Value> {
    match callback {
        Some(value) if super::call::is_callback(value) => Ok(value.clone()),
        _ => Err(fail(format!("{name} expects a function callback."), loc)),
    }
}
