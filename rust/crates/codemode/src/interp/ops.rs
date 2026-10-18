//! Purpose: operator semantics (binary, compound, unary, typeof, instanceof) with CodeMode's
//! data-only operand rule and JS coercions.
//! Scope: `runtime.ts` typeofValue/instanceofValue/applyBinaryOperator/evaluateUnaryExpression
//! (lines 260-330, 1597-1740).
//! Uses:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/number.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/expr.rs
//! `-- codemode/src/interp/stmt.rs
//! Invariants: opaque references never reach arithmetic; identity comparison never coerces.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

use crate::ast::Loc;
use crate::error::{DiagnosticKind, Eval, fail, fail_kind};
use crate::value::convert::{to_number, to_string};
use crate::value::number::{to_int32, to_uint32};
use crate::value::text::compare16;
use crate::value::{Coercion, GlobalName, Value, lock};

/// Opaque interpreter machinery anywhere inside `value` (sandbox values count as data).
pub(crate) fn contains_opaque(value: &Value) -> bool {
    fn walk(value: &Value, seen: &mut Vec<usize>) -> bool {
        if value.is_sandbox() {
            return false;
        }
        if value.is_opaque_reference() {
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
        let found = items.iter().any(|item| walk(item, seen));
        seen.pop();
        found
    }
    walk(value, &mut Vec::new())
}

pub(crate) fn typeof_value(value: &Value) -> &'static str {
    match value {
        Value::Undefined | Value::ShortCircuit => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::Str(_) => "string",
        Value::Tool(path) => {
            if path.is_empty() {
                "object"
            } else {
                "function"
            }
        }
        Value::Global(name) => match name {
            GlobalName::Math | GlobalName::Json | GlobalName::Console => "object",
            _ => "function",
        },
        Value::Function(_)
        | Value::Coercion(_)
        | Value::Intrinsic(_)
        | Value::GlobalMethod(_)
        | Value::PromiseMethod(_)
        | Value::PromiseNs
        | Value::ErrorCtor(_)
        | Value::Uri(_) => "function",
        _ => "object",
    }
}

pub(crate) fn instanceof_value(left: &Value, right: &Value, loc: Loc) -> Eval<bool> {
    match right {
        Value::ErrorCtor(name) => Ok(match left {
            Value::Object(object) => lock(object)
                .brand
                .is_some_and(|brand| *name == "Error" || brand == *name),
            _ => false,
        }),
        Value::Global(name) => Ok(match name {
            GlobalName::Date => matches!(left, Value::Date(_)),
            GlobalName::RegExp => matches!(left, Value::RegExp(_)),
            GlobalName::Map => matches!(left, Value::Map(_)),
            GlobalName::Set => matches!(left, Value::Set(_)),
            GlobalName::Url => matches!(left, Value::Url(_)),
            GlobalName::UrlSearchParams => matches!(left, Value::Params(_)),
            GlobalName::Array => matches!(left, Value::Array(_)),
            GlobalName::Object => left.is_object_like() || typeof_value(left) == "function",
            _ => return Err(instanceof_error(loc)),
        }),
        Value::PromiseNs => Ok(matches!(left, Value::Promise(_))),
        Value::Coercion(Coercion::Number | Coercion::String | Coercion::Boolean) => Ok(false),
        _ => Err(instanceof_error(loc)),
    }
}

fn instanceof_error(loc: Loc) -> crate::error::Fault {
    fail(
        "The right-hand side of 'instanceof' must be a constructor CodeMode knows: Error (or a specific error type like TypeError), Date, RegExp, Map, Set, URL, URLSearchParams, Array, Object, or Promise.",
        loc,
    )
}

/// ToPrimitive for operators: Dates use their string form for `+`, their time value otherwise;
/// other objects coerce to their JS string form.
fn coerce_operand(operand: &Value, operator: &str) -> Value {
    match operand {
        Value::Date(time) => {
            if operator == "+" {
                Value::str(to_string(operand))
            } else {
                Value::Number(**time)
            }
        }
        value if value.is_object_like() => Value::str(to_string(value)),
        value => value.clone(),
    }
}

/// Abstract loose equality over primitives (after object coercion).
fn loose_equals(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Undefined | Value::Null, Value::Undefined | Value::Null) => true,
        (Value::Undefined | Value::Null, _) | (_, Value::Undefined | Value::Null) => false,
        (Value::Str(_), Value::Str(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::Bool(_), Value::Bool(_)) => left.strict_equals(right),
        _ => to_number(left) == to_number(right),
    }
}

fn relational(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Str(left), Value::Str(right)) => Some(compare16(left, right)),
        _ => to_number(left).partial_cmp(&to_number(right)),
    }
}

/// Applies a binary operator to evaluated operands (shared with compound assignment).
pub(crate) fn binary(operator: &str, left: &Value, right: &Value, loc: Loc) -> Eval<Value> {
    if contains_opaque(left) || contains_opaque(right) {
        return Err(fail_kind(
            "Binary operators require data values in CodeMode.",
            loc,
            DiagnosticKind::InvalidDataValue,
        ));
    }
    let both_objects = left.is_object_like() && right.is_object_like();
    let l = coerce_operand(left, operator);
    let r = coerce_operand(right, operator);
    let num = |value: &Value| to_number(value);
    Ok(match operator {
        "+" => match (&l, &r) {
            (Value::Str(_), _) | (_, Value::Str(_)) => {
                Value::str(format!("{}{}", to_string(&l), to_string(&r)))
            }
            _ => Value::Number(num(&l) + num(&r)),
        },
        "-" => Value::Number(num(&l) - num(&r)),
        "*" => Value::Number(num(&l) * num(&r)),
        "/" => Value::Number(num(&l) / num(&r)),
        "%" => Value::Number(js_remainder(num(&l), num(&r))),
        "**" => Value::Number(js_pow(num(&l), num(&r))),
        "==" => Value::Bool(if both_objects {
            left.strict_equals(right)
        } else {
            loose_equals(&l, &r)
        }),
        "!=" => Value::Bool(
            !(if both_objects {
                left.strict_equals(right)
            } else {
                loose_equals(&l, &r)
            }),
        ),
        "===" => Value::Bool(left.strict_equals(right)),
        "!==" => Value::Bool(!left.strict_equals(right)),
        "<" => Value::Bool(relational(&l, &r) == Some(std::cmp::Ordering::Less)),
        "<=" => Value::Bool(matches!(
            relational(&l, &r),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        )),
        ">" => Value::Bool(relational(&l, &r) == Some(std::cmp::Ordering::Greater)),
        ">=" => Value::Bool(matches!(
            relational(&l, &r),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        )),
        "&" => Value::Number(f64::from(to_int32(num(&l)) & to_int32(num(&r)))),
        "|" => Value::Number(f64::from(to_int32(num(&l)) | to_int32(num(&r)))),
        "^" => Value::Number(f64::from(to_int32(num(&l)) ^ to_int32(num(&r)))),
        "<<" => Value::Number(f64::from(
            to_int32(num(&l)).wrapping_shl(to_uint32(num(&r)) & 31),
        )),
        ">>" => Value::Number(f64::from(to_int32(num(&l)) >> (to_uint32(num(&r)) & 31))),
        ">>>" => Value::Number(f64::from(to_uint32(num(&l)) >> (to_uint32(num(&r)) & 31))),
        "in" => {
            let key = to_string(&l);
            match right {
                Value::Object(object) => Value::Bool(lock(object).has(&key)),
                Value::Array(array) => {
                    let array = lock(array);
                    let index = key
                        .parse::<usize>()
                        .ok()
                        .filter(|_| crate::value::object::is_array_index(&key));
                    Value::Bool(
                        key == "length"
                            || index.is_some_and(|index| index < array.items.len())
                            || array.extra.contains_key(&key),
                    )
                }
                value if value.is_sandbox() => Value::Bool(false),
                _ => {
                    return Err(fail(
                        "The 'in' operator requires a data object on the right-hand side.",
                        loc,
                    ));
                }
            }
        }
        _ => {
            return Err(fail(
                format!("Unsupported binary operator '{operator}'."),
                loc,
            ));
        }
    })
}

fn js_remainder(left: f64, right: f64) -> f64 {
    if right.is_infinite() && left.is_finite() {
        return left;
    }
    left % right
}

fn js_pow(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() || ((base == 1.0 || base == -1.0) && exponent.is_infinite()) {
        return f64::NAN;
    }
    base.powf(exponent)
}

pub(crate) fn unary(operator: &str, value: &Value, loc: Loc) -> Eval<Value> {
    if contains_opaque(value) {
        return Err(fail_kind(
            "Unary operators require data values in CodeMode.",
            loc,
            DiagnosticKind::InvalidDataValue,
        ));
    }
    let operand = match value {
        Value::Date(time) => **time,
        value if value.is_object_like() => to_number(&Value::str(to_string(value))),
        value => to_number(value),
    };
    Ok(Value::Number(match operator {
        "+" => operand,
        "-" => -operand,
        "~" => f64::from(!to_int32(operand)),
        _ => {
            return Err(fail(
                format!("Unsupported unary operator '{operator}'."),
                loc,
            ));
        }
    }))
}

pub(crate) const COMPOUND_OPERATORS: [&str; 12] = [
    "+=", "-=", "*=", "/=", "%=", "**=", "&=", "|=", "^=", "<<=", ">>=", ">>>=",
];

pub(crate) fn compound(operator: &str, current: &Value, incoming: &Value, loc: Loc) -> Eval<Value> {
    if !COMPOUND_OPERATORS.contains(&operator) {
        return Err(fail(
            format!("Unsupported assignment operator '{operator}'."),
            loc,
        ));
    }
    binary(&operator[..operator.len() - 1], current, incoming, loc)
}
