//! Purpose: member access: reference resolution over every value kind, reads, and the single
//! write path (plain/compound/logical assignment and updates).
//! Scope: `runtime.ts` getMemberReference/readMember/modifyMember/assignToReference
//! (lines 2905-3330).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/stdlib/mod.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/expr.rs
//! Invariants: blocked keys never resolve; promises and runtime references expose no properties;
//! each member object/key expression is evaluated exactly once per access.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/promise.rs, codemode/tests/stdlib.rs

use super::Interp;
use crate::ast::{Loc, Member, MemberProp};
use crate::error::{DiagnosticKind, Eval, fail, fail_kind, unsupported_feature};
use crate::stdlib::{date, math, number, regexp, string, url};
use crate::value::convert::is_blocked_member;
use crate::value::text::{len16, utf16};
use crate::value::{Coercion, GlobalName, PromiseMethod, Value, lock};
use std::sync::Arc;

pub(crate) enum Key {
    Str(String),
    Num(f64),
}

impl Key {
    pub fn into_string(self) -> String {
        match self {
            Self::Str(text) => text,
            Self::Num(number) => crate::value::number::format(number),
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            Self::Num(_) => None,
        }
    }

    fn index(&self) -> Option<usize> {
        match self {
            Self::Num(number) if *number >= 0.0 && number.fract() == 0.0 => Some(*number as usize),
            Self::Str(text)
                if text.bytes().all(|byte| byte.is_ascii_digit()) && !text.is_empty() =>
            {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| *value < 4_294_967_295.0)
                    .map(|value| value as usize)
            }
            _ => None,
        }
    }
}

pub(crate) fn property_key(value: &Value, loc: Loc) -> Eval<Key> {
    match value {
        Value::Str(text) => Ok(Key::Str(text.to_string())),
        Value::Number(number) => Ok(Key::Num(*number)),
        _ => Err(fail("Property key must be a string or number.", loc)),
    }
}

pub(super) enum Reference {
    Short,
    Value(Value),
    Slot { target: Value, key: Key },
}

const UNDEFINED: Reference = Reference::Value(Value::Undefined);

impl Interp {
    pub(super) async fn member_reference(&mut self, member: &Member) -> Eval<Reference> {
        let object = self.expr(&member.object).await?;
        if matches!(object, Value::ShortCircuit) || (object.is_nullish() && member.optional) {
            return Ok(Reference::Short);
        }
        let (key, property_loc) = match &member.property {
            MemberProp::Static(name, loc) => (Key::Str(name.clone()), *loc),
            MemberProp::Computed(expression, loc) => {
                let value = self.expr(expression).await?;
                (property_key(&value, *loc)?, *loc)
            }
        };
        resolve_reference(object, key, member.object.loc(), property_loc)
    }

    pub(super) async fn read_member(&mut self, member: &Member) -> Eval<Value> {
        let reference = self.member_reference(member).await?;
        self.slot_current(&reference, member)
    }

    pub(super) fn slot_current(&self, reference: &Reference, _member: &Member) -> Eval<Value> {
        Ok(match reference {
            Reference::Short => Value::ShortCircuit,
            Reference::Value(value) => value.clone(),
            Reference::Slot {
                target: Value::Array(array),
                key,
            } => {
                if let Some(name) = key
                    .as_str()
                    .filter(|name| crate::stdlib::ARRAY_METHODS.contains(name))
                {
                    return Ok(Value::Intrinsic(Arc::new((
                        Value::Array(array.clone()),
                        name.to_string(),
                    ))));
                }
                let array = lock(array);
                if key.as_str() == Some("length") {
                    return Ok(Value::Number(array.items.len() as f64));
                }
                key.index()
                    .and_then(|index| array.items.get(index).cloned())
                    .unwrap_or(Value::Undefined)
            }
            Reference::Slot {
                target: Value::Url(data),
                key,
            } => url::get_property(&lock(data), key.as_str().unwrap_or("")),
            Reference::Slot {
                target: Value::Object(object),
                key,
            } => {
                let key = match key {
                    Key::Str(text) => text.clone(),
                    Key::Num(number) => crate::value::number::format(*number),
                };
                lock(object).get(&key).cloned().unwrap_or(Value::Undefined)
            }
            Reference::Slot { .. } => Value::Undefined,
        })
    }

    /// Resolves the member once, then lets `compute` decide whether to write.
    pub(super) async fn modify_member(
        &mut self,
        member: &Member,
        compute: impl FnOnce(Value) -> Eval<Option<(Value, Value)>>,
    ) -> Eval<Value> {
        let reference = self.member_reference(member).await?;
        let current = self.slot_current(&reference, member)?;
        match compute(current.clone())? {
            Some((next, result)) => {
                self.slot_write(reference, member, next)?;
                Ok(result)
            }
            None => Ok(current),
        }
    }

    pub(super) fn slot_write(
        &mut self,
        reference: Reference,
        member: &Member,
        next: Value,
    ) -> Eval<()> {
        let Reference::Slot { target, key } = reference else {
            return Err(fail(
                "Only data fields may be assigned in CodeMode.",
                member.loc,
            ));
        };
        match &target {
            Value::Array(array) => {
                if key.as_str() == Some("length") {
                    return Err(fail(
                        "Array length cannot be assigned in CodeMode.",
                        member.loc,
                    ));
                }
                if key
                    .as_str()
                    .is_some_and(|name| crate::stdlib::ARRAY_METHODS.contains(&name))
                {
                    return Err(fail(
                        "Array methods cannot be assigned in CodeMode.",
                        member.loc,
                    ));
                }
                let Some(index) = key.index() else {
                    return Err(fail_kind(
                        "Array assignment index must be a non-negative integer.",
                        member.loc,
                        DiagnosticKind::InvalidDataValue,
                    ));
                };
                reject_circular_insertion(&target, &next, "Array assignment result", member.loc)?;
                let mut array = lock(array);
                if index >= array.items.len() {
                    array.items.resize(index + 1, Value::Undefined);
                }
                array.items[index] = next;
                Ok(())
            }
            Value::Url(data) => {
                url::set_property(data, key.as_str().unwrap_or(""), &next, member.loc)
            }
            Value::Object(object) => {
                reject_circular_insertion(&target, &next, "Object assignment result", member.loc)?;
                lock(object).set(key.into_string(), next);
                Ok(())
            }
            _ => Err(fail(
                "Only data fields may be assigned in CodeMode.",
                member.loc,
            )),
        }
    }
}

/// Rejects inserting a value that (transitively) contains `container`.
pub(crate) fn reject_circular_insertion(
    container: &Value,
    value: &Value,
    label: &str,
    loc: Loc,
) -> Eval<()> {
    fn walk(container: usize, value: &Value, seen: &mut Vec<usize>) -> bool {
        let Some(id) = value.identity() else {
            return false;
        };
        if id == container {
            return true;
        }
        if value.is_runtime_reference() || seen.contains(&id) {
            return false;
        }
        seen.push(id);
        let items: Vec<Value> = match value {
            Value::Array(array) => lock(array).items.clone(),
            Value::Object(object) => lock(object).props.values().cloned().collect(),
            _ => Vec::new(),
        };
        let found = items.iter().any(|item| walk(container, item, seen));
        seen.pop();
        found
    }
    let Some(container) = container.identity() else {
        return Ok(());
    };
    if walk(container, value, &mut Vec::new()) {
        return Err(fail_kind(
            format!("{label} contains a circular value."),
            loc,
            DiagnosticKind::InvalidDataValue,
        ));
    }
    Ok(())
}

fn intrinsic(receiver: Value, name: &str) -> Reference {
    Reference::Value(Value::Intrinsic(Arc::new((receiver, name.to_string()))))
}

fn resolve_reference(
    object: Value,
    key: Key,
    object_loc: Loc,
    property_loc: Loc,
) -> Eval<Reference> {
    let name = key.as_str().map(str::to_string);
    let named = |set: &[&str]| name.as_deref().is_some_and(|name| set.contains(&name));
    match &object {
        Value::Tool(path) => {
            let Some(name) = name.filter(|name| !is_blocked_member(name)) else {
                return Err(fail(
                    "Tool paths must use safe string property names.",
                    property_loc,
                ));
            };
            let mut next = path.as_ref().clone();
            next.push(name);
            return Ok(Reference::Value(Value::Tool(Arc::new(next))));
        }
        Value::PromiseNs => {
            return match name.as_deref().and_then(PromiseMethod::parse) {
                Some(method) => Ok(Reference::Value(Value::PromiseMethod(method))),
                None => Err(fail(
                    format!(
                        "Promise.{} is not available in CodeMode. Available: Promise.all, Promise.allSettled, Promise.race, Promise.resolve, and Promise.reject; consume promises with await.",
                        key.into_string()
                    ),
                    property_loc,
                )),
            };
        }
        Value::Global(global) => {
            let Some(name) = name.filter(|name| !is_blocked_member(name)) else {
                return Err(fail(
                    format!(
                        "{}.{} is not available in CodeMode.",
                        global.as_str(),
                        key.into_string()
                    ),
                    property_loc,
                ));
            };
            if *global == GlobalName::Math
                && let Some(constant) = math::constant(&name)
            {
                return Ok(Reference::Value(Value::Number(constant)));
            }
            return Ok(Reference::Value(Value::GlobalMethod(Arc::new((
                global.as_str().to_string(),
                name,
            )))));
        }
        Value::Str(text) => {
            return Ok(match &key {
                Key::Str(name) if name == "length" => {
                    Reference::Value(Value::Number(len16(text) as f64))
                }
                Key::Str(name) if string::METHODS.contains(&name.as_str()) => {
                    intrinsic(object.clone(), name)
                }
                _ => match key.index().filter(|_| {
                    matches!(&key, Key::Num(_))
                        || name
                            .as_deref()
                            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
                }) {
                    Some(index) => {
                        Reference::Value(utf16(text).get(index).map_or(Value::Undefined, |unit| {
                            Value::str(String::from_utf16_lossy(&[*unit]))
                        }))
                    }
                    None => UNDEFINED,
                },
            });
        }
        Value::Number(_) => {
            return Ok(if named(number::METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::Coercion(coercion)
            if name.as_deref().is_some_and(|name| !is_blocked_member(name)) =>
        {
            let name = name.clone().unwrap_or_default();
            if *coercion == Coercion::Number {
                if let Some(constant) = number::constant(&name) {
                    return Ok(Reference::Value(Value::Number(constant)));
                }
                if number::STATICS.contains(&name.as_str()) {
                    return Ok(Reference::Value(Value::GlobalMethod(Arc::new((
                        "Number".into(),
                        name,
                    )))));
                }
            }
            if *coercion == Coercion::String && string::STATICS.contains(&name.as_str()) {
                return Ok(Reference::Value(Value::GlobalMethod(Arc::new((
                    "String".into(),
                    name,
                )))));
            }
        }
        Value::Date(_) => {
            return Ok(if named(date::METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::RegExp(regex) => {
            if let Some(value) = name
                .as_deref()
                .and_then(|name| regexp::property(regex, name))
            {
                return Ok(Reference::Value(value));
            }
            return Ok(if named(regexp::METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::Map(map) => {
            if name.as_deref() == Some("size") {
                return Ok(Reference::Value(Value::Number(
                    lock(map).entries.len() as f64
                )));
            }
            return Ok(if named(crate::stdlib::MAP_METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::Set(set) => {
            if name.as_deref() == Some("size") {
                return Ok(Reference::Value(Value::Number(
                    lock(set).entries.len() as f64
                )));
            }
            return Ok(if named(crate::stdlib::SET_METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::Url(data) => {
            if name.as_deref() == Some("searchParams") {
                return Ok(Reference::Value(Value::Params(lock(data).params.clone())));
            }
            if named(url::URL_METHODS) {
                return Ok(intrinsic(object.clone(), name.as_deref().unwrap_or("")));
            }
            if named(url::URL_PROPERTIES) {
                return Ok(Reference::Slot {
                    target: object.clone(),
                    key,
                });
            }
            return Ok(UNDEFINED);
        }
        Value::Params(params) => {
            if name.as_deref() == Some("size") {
                return Ok(Reference::Value(Value::Number(
                    lock(params).pairs.len() as f64
                )));
            }
            return Ok(if named(url::PARAMS_METHODS) {
                intrinsic(object.clone(), name.as_deref().unwrap_or(""))
            } else {
                UNDEFINED
            });
        }
        Value::Promise(_) => {
            if let Some(method @ ("then" | "catch" | "finally")) = name.as_deref() {
                return Err(unsupported_feature(
                    format!(
                        "Promise.prototype.{method} is not supported in CodeMode; use await instead (with try/catch to handle failures) - e.g. `const result = await tools.ns.tool(...)`."
                    ),
                    property_loc,
                ));
            }
            return Err(fail_kind(
                "This value is an un-awaited Promise and has no readable properties; await it first - e.g. `const result = await tools.ns.tool(...)`.",
                object_loc,
                DiagnosticKind::InvalidDataValue,
            ));
        }
        _ => {}
    }
    if object.is_runtime_reference() {
        return Err(fail_kind(
            "CodeMode runtime references are opaque and do not expose properties.",
            object_loc,
            DiagnosticKind::InvalidDataValue,
        ));
    }
    let (Value::Array(_) | Value::Object(_)) = &object else {
        return Err(fail(
            "Cannot access a property on a non-object value.",
            object_loc,
        ));
    };
    if let Some(name) = name.as_deref().filter(|name| is_blocked_member(name)) {
        return Err(fail(
            format!("Property '{name}' is not available in CodeMode."),
            property_loc,
        ));
    }
    if let Value::Array(array) = &object {
        let indexable = matches!(key, Key::Num(_))
            || named(&["length"])
            || named(crate::stdlib::ARRAY_METHODS)
            || name.as_deref().is_some_and(|name| {
                name.bytes().all(|byte| byte.is_ascii_digit()) && !name.is_empty()
            });
        if !indexable {
            let extra = name
                .as_deref()
                .and_then(|name| lock(array).extra.get(name).cloned());
            return Ok(Reference::Value(extra.unwrap_or(Value::Undefined)));
        }
    }
    Ok(Reference::Slot {
        target: object,
        key,
    })
}
