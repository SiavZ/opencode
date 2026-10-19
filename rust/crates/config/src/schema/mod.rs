//! Purpose: minimal Effect-Schema-compatible structural decoder used for V1 and V2 config
//! shapes: `errors: "all"`, `onExcessProperty: "ignore"`, `propertyOrder: "original"`.
//! Scope: pure decoding.
//! Uses:
//! rust/crates/
//! |-- config/src/error.rs
//! |-- config/src/schema/v1.rs
//! |-- config/src/schema/v1_provider.rs
//! `-- config/src/schema/v2.rs
//! External:
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/compat_mcp.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/steps.rs
//! |-- config/src/legacy/update.rs
//! |-- config/src/lib.rs
//! |-- config/src/resources.rs
//! |-- config/src/schema/v1.rs
//! |-- config/src/schema/v1_provider.rs
//! |-- config/src/schema/v2.rs
//! `-- config/tests/compat_fixtures.rs
//! Invariants: struct output keeps input key order and drops unknown keys unless a rest record is
//! declared; unions pick the first member that decodes; transforms run after a successful decode.
//! Contract tests: config/tests/compat_fixtures.rs

pub mod v1;
mod v1_provider;
pub mod v2;

use crate::error::Issue;
use serde_json::{Map, Value};

pub type Transform = fn(Value) -> Value;
pub type Check = fn(&Value) -> Option<String>;

#[derive(Clone)]
pub enum Shape {
    Any,
    String,
    Bool,
    Finite,
    Int,
    PositiveInt,
    NonNegativeInt,
    IntBetween(i64, i64),
    Literals(&'static [&'static str]),
    False,
    Pattern(fn(&str) -> bool, &'static str),
    Array(Box<Shape>),
    Record(Box<Shape>),
    Struct(Vec<Field>, Option<Box<Shape>>),
    Union(Vec<Shape>),
    Map(Box<Shape>, Transform),
    Check(Box<Shape>, Check),
}

impl std::fmt::Debug for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shape")
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: &'static str,
    pub shape: Shape,
    pub optional: bool,
}

pub fn opt(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        optional: true,
    }
}

pub fn req(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        optional: false,
    }
}

pub fn strukt(fields: Vec<Field>) -> Shape {
    Shape::Struct(fields, None)
}

pub fn array(shape: Shape) -> Shape {
    Shape::Array(Box::new(shape))
}

pub fn record(shape: Shape) -> Shape {
    Shape::Record(Box::new(shape))
}

pub fn union(shapes: Vec<Shape>) -> Shape {
    Shape::Union(shapes)
}

/// Decode `value` with `shape`, returning every issue on failure.
pub fn decode(shape: &Shape, value: &Value) -> Result<Value, Vec<Issue>> {
    let mut issues = Vec::new();
    let out = walk(shape, value, &mut Vec::new(), &mut issues);
    match out {
        Some(out) if issues.is_empty() => Ok(out),
        _ => Err(issues),
    }
}

pub fn is_valid(shape: &Shape, value: &Value) -> bool {
    decode(shape, value).is_ok()
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => format!("{text:?}"),
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
        other => other.to_string(),
    }
}

fn fail(path: &[String], expected: &str, value: &Value, issues: &mut Vec<Issue>) -> Option<Value> {
    issues.push(Issue::new(
        path,
        format!("Expected {expected}, got {}", describe(value)),
    ));
    None
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn is_integer(value: &Value) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|n| n.is_finite() && n.fract() == 0.0)
}

fn walk(
    shape: &Shape,
    value: &Value,
    path: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<Value> {
    match shape {
        Shape::Any => Some(value.clone()),
        Shape::String => value
            .is_string()
            .then(|| value.clone())
            .or_else(|| fail(path, "string", value, issues)),
        Shape::Bool => value
            .is_boolean()
            .then(|| value.clone())
            .or_else(|| fail(path, "boolean", value, issues)),
        Shape::Finite => number(value)
            .filter(|n| n.is_finite())
            .map(|_| value.clone())
            .or_else(|| fail(path, "a finite number", value, issues)),
        Shape::Int => {
            if value.is_number() && is_integer(value) {
                return Some(value.clone());
            }
            fail(path, "an integer", value, issues)
        }
        Shape::PositiveInt | Shape::NonNegativeInt => {
            let base = walk(&Shape::Int, value, path, issues)?;
            let n = number(&base).unwrap_or_default();
            let positive = matches!(shape, Shape::PositiveInt);
            if (positive && n > 0.0) || (!positive && n >= 0.0) {
                return Some(base);
            }
            let bound = if positive {
                "a value greater than 0"
            } else {
                "a value greater than or equal to 0"
            };
            fail(path, bound, value, issues)
        }
        Shape::IntBetween(min, max) => {
            let base = walk(&Shape::Int, value, path, issues)?;
            let n = number(&base).unwrap_or_default();
            if n >= *min as f64 && n <= *max as f64 {
                return Some(base);
            }
            fail(
                path,
                &format!("a value between {min} and {max}"),
                value,
                issues,
            )
        }
        Shape::Literals(options) => {
            if value.as_str().is_some_and(|text| options.contains(&text)) {
                return Some(value.clone());
            }
            let expected = options
                .iter()
                .map(|o| format!("{o:?}"))
                .collect::<Vec<_>>()
                .join(" | ");
            fail(path, &expected, value, issues)
        }
        Shape::False => (value == &Value::Bool(false))
            .then(|| value.clone())
            .or_else(|| fail(path, "false", value, issues)),
        Shape::Pattern(test, description) => {
            let base = walk(&Shape::String, value, path, issues)?;
            if base.as_str().is_some_and(test) {
                return Some(base);
            }
            fail(
                path,
                &format!("a string matching the pattern {description}"),
                value,
                issues,
            )
        }
        Shape::Array(item) => {
            let Some(items) = value.as_array() else {
                return fail(path, "array", value, issues);
            };
            let mut out = Vec::new();
            let mut ok = true;
            for (index, entry) in items.iter().enumerate() {
                path.push(index.to_string());
                match walk(item, entry, path, issues) {
                    Some(decoded) => out.push(decoded),
                    None => ok = false,
                }
                path.pop();
            }
            ok.then_some(Value::Array(out))
        }
        Shape::Record(item) => {
            let Some(map) = value.as_object() else {
                return fail(path, "object", value, issues);
            };
            decode_entries(map, |_| Some(item.as_ref()), path, issues)
        }
        Shape::Struct(fields, rest) => decode_struct(fields, rest.as_deref(), value, path, issues),
        Shape::Union(members) => {
            for member in members {
                let mut scratch = Vec::new();
                if let Some(decoded) = walk(member, value, path, &mut scratch)
                    && scratch.is_empty()
                {
                    return Some(decoded);
                }
            }
            let mut best = Vec::new();
            for member in members {
                let mut scratch = Vec::new();
                walk(member, value, path, &mut scratch);
                if best.is_empty() || (scratch.len() < best.len() && !scratch.is_empty()) {
                    best = scratch;
                }
            }
            if best.is_empty() {
                return fail(path, "a matching union member", value, issues);
            }
            issues.extend(best);
            None
        }
        Shape::Map(inner, transform) => walk(inner, value, path, issues).map(transform),
        Shape::Check(inner, check) => {
            let decoded = walk(inner, value, path, issues)?;
            match check(&decoded) {
                None => Some(decoded),
                Some(message) => {
                    issues.push(Issue::new(path, message));
                    None
                }
            }
        }
    }
}

fn decode_entries<'s>(
    map: &Map<String, Value>,
    lookup: impl Fn(&str) -> Option<&'s Shape>,
    path: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<Value> {
    let mut out = Map::new();
    let mut ok = true;
    for (key, entry) in map {
        let Some(shape) = lookup(key) else { continue };
        path.push(key.clone());
        match walk(shape, entry, path, issues) {
            Some(decoded) => {
                out.insert(key.clone(), decoded);
            }
            None => ok = false,
        }
        path.pop();
    }
    ok.then_some(Value::Object(out))
}

fn decode_struct(
    fields: &[Field],
    rest: Option<&Shape>,
    value: &Value,
    path: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<Value> {
    let Some(map) = value.as_object().filter(|_| !value.is_array()) else {
        return fail(path, "object", value, issues);
    };
    let mut ok = true;
    for field in fields
        .iter()
        .filter(|field| !field.optional && !map.contains_key(field.name))
    {
        path.push(field.name.to_string());
        issues.push(Issue::new(path, "Missing key"));
        path.pop();
        ok = false;
    }
    let decoded = decode_entries(
        map,
        |key| {
            fields
                .iter()
                .find(|field| field.name == key)
                .map(|field| &field.shape)
                .or(rest)
        },
        path,
        issues,
    );
    if ok { decoded } else { None }
}
