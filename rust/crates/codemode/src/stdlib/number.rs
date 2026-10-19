//! Purpose: `Number` methods, statics and constants.
//! Scope: stdlib/number.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/number.rs
//! External:
//! |-- f64::EPSILON
//! |-- f64::MAX
//! |-- f64::is_finite
//! `-- f64::is_nan
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/members.rs
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: method arguments must be numbers; radix is 2..=36.
//! Contract tests: codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{Eval, fail, fail_named};
use crate::value::Value;
use crate::value::convert::to_string;
use crate::value::number::{
    format, is_safe_integer, parse_float, parse_int, to_exponential, to_fixed, to_precision,
    to_radix_string,
};

pub(crate) const METHODS: &[&str] = &["toFixed", "toPrecision", "toExponential", "toString"];
pub(crate) const STATICS: &[&str] = &[
    "isInteger",
    "isFinite",
    "isNaN",
    "isSafeInteger",
    "parseInt",
    "parseFloat",
];

pub(crate) fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "MAX_SAFE_INTEGER" => 9_007_199_254_740_991.0,
        "MIN_SAFE_INTEGER" => -9_007_199_254_740_991.0,
        "MAX_VALUE" => f64::MAX,
        "MIN_VALUE" => 5e-324,
        "EPSILON" => f64::EPSILON,
        _ => return None,
    })
}

pub(crate) fn invoke_method(value: f64, name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    let optional = |index: usize| -> Eval<Option<f64>> {
        match args.get(index) {
            None | Some(Value::Undefined) => Ok(None),
            Some(Value::Number(number)) => Ok(Some(*number)),
            Some(_) => Err(fail(
                format!("Number.{name} expects a number argument."),
                loc,
            )),
        }
    };
    let digits = |count: f64, minimum: f64| -> Eval<usize> {
        if !(minimum..=100.0).contains(&count.trunc()) {
            return Err(fail_named(
                format!("{name}() argument must be between {minimum} and 100"),
                loc,
                "RangeError",
            ));
        }
        Ok(count.trunc() as usize)
    };
    Ok(Value::str(match name {
        "toFixed" => to_fixed(value, digits(optional(0)?.unwrap_or(0.0), 0.0)?),
        "toExponential" => match optional(0)? {
            Some(count) => to_exponential(value, Some(digits(count, 0.0)?)),
            None => to_exponential(value, None),
        },
        "toPrecision" => match optional(0)? {
            Some(count) => to_precision(value, digits(count, 1.0)?),
            None => format(value),
        },
        "toString" => match optional(0)? {
            Some(radix) if !(2.0..=36.0).contains(&radix) => {
                return Err(fail("Number.toString radix must be between 2 and 36.", loc));
            }
            Some(radix) => to_radix_string(value, radix as u32),
            None => format(value),
        },
        _ => {
            return Err(fail(
                format!("Number method '{name}' is not available in CodeMode."),
                loc,
            ));
        }
    }))
}

pub(crate) fn invoke_static(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    let number = match value {
        Value::Number(number) => Some(number),
        _ => None,
    };
    Ok(match name {
        "isInteger" => {
            Value::Bool(number.is_some_and(|number| number.is_finite() && number.trunc() == number))
        }
        "isFinite" => Value::Bool(number.is_some_and(f64::is_finite)),
        "isNaN" => Value::Bool(number.is_some_and(f64::is_nan)),
        "isSafeInteger" => Value::Bool(number.is_some_and(is_safe_integer)),
        "parseInt" => {
            let radix = match args.get(1) {
                None | Some(Value::Undefined) => None,
                Some(Value::Number(radix)) => Some(*radix),
                Some(_) => return Err(fail("Number.parseInt expects a numeric radix.", loc)),
            };
            Value::Number(parse_int(&to_string(&value), radix))
        }
        "parseFloat" => Value::Number(parse_float(&to_string(&value))),
        _ => {
            return Err(fail(
                format!("Number.{name} is not available in CodeMode."),
                loc,
            ));
        }
    })
}
