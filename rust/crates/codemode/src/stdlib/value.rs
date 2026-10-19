//! Purpose: global coercion functions (`Number`, `String`, `Boolean`, `parseInt`, `parseFloat`).
//! Scope: stdlib/value.ts invokeCoercion.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/number.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: inputs pass the intra-sandbox data checkpoint (promises are rejected).
//! Contract tests: codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::convert::{bounded, to_number, to_string};
use crate::value::number::{parse_float, parse_int};
use crate::value::{Coercion, Value};

pub(crate) fn invoke_coercion(coercion: Coercion, args: &[Value], loc: Loc) -> Eval<Value> {
    let raw = args.first().cloned().unwrap_or(Value::Undefined);
    if raw.is_sandbox() {
        return Ok(match coercion {
            Coercion::Boolean => Value::Bool(true),
            Coercion::Number => Value::Number(to_number(&raw)),
            Coercion::String => Value::str(to_string(&raw)),
            Coercion::ParseInt => Value::Number(parse_int(&to_string(&raw), None)),
            Coercion::ParseFloat => Value::Number(parse_float(&to_string(&raw))),
        });
    }
    let value = bounded(&raw, &format!("{} input", coercion.as_str()))?;
    Ok(match coercion {
        Coercion::Number => Value::Number(to_number(&value)),
        Coercion::Boolean => Value::Bool(value.truthy()),
        Coercion::String => Value::str(to_string(&value)),
        Coercion::ParseFloat => Value::Number(parse_float(&to_string(&value))),
        Coercion::ParseInt => {
            let radix = match args.get(1) {
                None | Some(Value::Undefined) => None,
                Some(Value::Number(radix)) => Some(*radix),
                Some(_) => return Err(fail("parseInt expects a numeric radix.", loc)),
            };
            Value::Number(parse_int(&to_string(&value), radix))
        }
    })
}
