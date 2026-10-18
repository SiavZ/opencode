//! Purpose: `Math` constants and allowlisted static functions.
//! Scope: stdlib/math.ts.
//! Uses:
//! rust/crates/
//! `-- codemode/src/value/number.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/members.rs
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: arguments must be numbers; no randomness or host state is exposed.
//! Contract tests: codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::Value;
use crate::value::number::js_round;
use std::f64::consts;

pub(crate) fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "PI" => consts::PI,
        "E" => consts::E,
        "LN2" => consts::LN_2,
        "LN10" => consts::LN_10,
        "LOG2E" => consts::LOG2_E,
        "LOG10E" => consts::LOG10_E,
        "SQRT2" => consts::SQRT_2,
        "SQRT1_2" => consts::FRAC_1_SQRT_2,
        _ => return None,
    })
}

const METHODS: &[&str] = &[
    "max", "min", "abs", "floor", "ceil", "round", "trunc", "sign", "sqrt", "cbrt", "pow", "hypot",
    "log", "log2", "log10", "exp",
];

pub(crate) fn invoke(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    if !METHODS.contains(&name) {
        return Err(fail(
            format!("Math.{name} is not available in CodeMode."),
            loc,
        ));
    }
    let nums = args
        .iter()
        .map(|arg| match arg {
            Value::Number(number) => Ok(*number),
            _ => Err(fail(format!("Math.{name} expects number arguments."), loc)),
        })
        .collect::<Eval<Vec<f64>>>()?;
    let a = nums.first().copied().unwrap_or(f64::NAN);
    let b = nums.get(1).copied().unwrap_or(f64::NAN);
    Ok(Value::Number(match name {
        "max" => nums.iter().copied().fold(f64::NEG_INFINITY, |acc, item| {
            if acc.is_nan() || item.is_nan() {
                f64::NAN
            } else {
                acc.max(item)
            }
        }),
        "min" => nums.iter().copied().fold(f64::INFINITY, |acc, item| {
            if acc.is_nan() || item.is_nan() {
                f64::NAN
            } else {
                acc.min(item)
            }
        }),
        "abs" => a.abs(),
        "floor" => a.floor(),
        "ceil" => a.ceil(),
        "round" => js_round(a),
        "trunc" => a.trunc(),
        "sign" => {
            if a.is_nan() || a == 0.0 {
                a
            } else {
                a.signum()
            }
        }
        "sqrt" => a.sqrt(),
        "cbrt" => a.cbrt(),
        "pow" => {
            if b.is_nan() || ((a == 1.0 || a == -1.0) && b.is_infinite()) {
                f64::NAN
            } else {
                a.powf(b)
            }
        }
        "hypot" => {
            if nums.iter().any(|item| item.is_infinite()) {
                f64::INFINITY
            } else {
                nums.iter().map(|item| item * item).sum::<f64>().sqrt()
            }
        }
        "log" => a.ln(),
        "log2" => a.log2(),
        "log10" => a.log10(),
        _ => a.exp(),
    }))
}
