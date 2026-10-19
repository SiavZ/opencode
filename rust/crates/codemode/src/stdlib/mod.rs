//! Purpose: standard-library registry and global static dispatch (`Object.*`, `Math.*`,
//! `Array.*`, `Number.*`, `String.*`, `URL.*`, `Date.*`, `JSON.*`).
//! Scope: allowlists and routing; behavior lives in the per-type modules.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/stdlib/array.rs
//! |-- codemode/src/stdlib/collections.rs
//! |-- codemode/src/stdlib/date.rs
//! |-- codemode/src/stdlib/json.rs
//! |-- codemode/src/stdlib/math.rs
//! |-- codemode/src/stdlib/number.rs
//! |-- codemode/src/stdlib/object.rs
//! |-- codemode/src/stdlib/regexp.rs
//! |-- codemode/src/stdlib/string.rs
//! |-- codemode/src/stdlib/url.rs
//! |-- codemode/src/stdlib/value.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/members.rs
//! `-- codemode/src/lib.rs
//! Invariants: only allowlisted names are callable; everything else is a clear diagnostic.
//! Contract tests: codemode/tests/stdlib.rs, codemode/tests/parity.rs

pub(crate) mod array;
pub(crate) mod collections;
pub(crate) mod date;
pub(crate) mod json;
pub(crate) mod math;
pub(crate) mod number;
pub(crate) mod object;
pub(crate) mod regexp;
pub(crate) mod string;
pub(crate) mod url;
pub(crate) mod value;

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::Value;

pub(crate) const ARRAY_METHODS: &[&str] = &[
    "map",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "some",
    "every",
    "includes",
    "join",
    "reduce",
    "reduceRight",
    "flatMap",
    "forEach",
    "sort",
    "toSorted",
    "slice",
    "concat",
    "indexOf",
    "lastIndexOf",
    "at",
    "flat",
    "reverse",
    "toReversed",
    "with",
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "fill",
    "copyWithin",
    "keys",
    "values",
    "entries",
];

pub(crate) const MAP_METHODS: &[&str] = &[
    "get", "set", "has", "delete", "clear", "forEach", "keys", "values", "entries",
];

pub(crate) const SET_METHODS: &[&str] = &[
    "add", "has", "delete", "clear", "forEach", "keys", "values", "entries",
];

pub(crate) fn invoke_global(namespace: &str, name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    match namespace {
        "Object" => object::invoke(name, args, loc),
        "Math" => math::invoke(name, args, loc),
        "Array" => array::invoke_static(name, args, loc),
        "Number" => number::invoke_static(name, args, loc),
        "String" => string::invoke_static(name, args, loc),
        "URL" => url::invoke_static(name, args, loc),
        "Date" => date::invoke_static(name, args, loc),
        "JSON" => json::invoke(name, args, loc),
        other => Err(fail(
            format!("{other}.{name} is not available in CodeMode."),
            loc,
        )),
    }
}
