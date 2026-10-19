//! Purpose: shared helpers for the current registry conformance tests: baseline OpenAPI loading
//! and a minimal Effect-style JSON Schema rendering of the protocol IR.
//! Scope: test support, fixture-based.
//! Uses:
//! rust/crates/
//! `-- protocol/src/ir.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- protocol/tests/current_registry.rs
//! Invariants: `render` mirrors the baseline renderer only as far as the current API needs.
//! Contract tests: protocol/tests/current_registry.rs

use opencode_protocol::ir::{Optionality, Schema};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Event unions owned by p06.events-openapi; checked there.
pub const EVENT_OWNED: &[&str] = &[
    "V2Event",
    "SessionDurableEvent",
    "SessionDurableEventStream",
];

/// Operations whose baseline OpenAPI rendering intentionally differs from the IR (reason).
pub const KNOWN_DIVERGENCES: &[(&str, &str)] = &[
    (
        "v2.integration.get",
        "UndefinedOr data renders required; JSON omits undefined so IR marks it optional",
    ),
    (
        "v2.pty.connect",
        "WebSocket upgrade; OpenAPI renders a 200 boolean and transform-added raw query params",
    ),
];

pub fn baseline() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../packages/sdk/openapi.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read baseline openapi.json"))
        .expect("parse baseline openapi.json")
}

pub fn component_key(name: &str) -> &str {
    // The baseline document merges the legacy API first, so the current `UnknownError` envelope
    // was suffixed by the renderer.
    if name == "UnknownError" {
        "UnknownError1"
    } else {
        name
    }
}

pub fn reference(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{}", component_key(name)) })
}

pub fn checks(out: &mut Map<String, Value>, c: &opencode_protocol::ir::Checks) {
    let num = |v: f64| {
        if v.fract() == 0.0 {
            json!(v as i64)
        } else {
            json!(v)
        }
    };
    if let Some(p) = &c.pattern {
        out.insert("pattern".into(), json!(p));
    }
    for (k, v) in [
        ("minimum", c.minimum),
        ("exclusiveMinimum", c.exclusive_minimum),
        ("maximum", c.maximum),
        ("exclusiveMaximum", c.exclusive_maximum),
    ] {
        if let Some(v) = v {
            out.insert(k.into(), num(v));
        }
    }
}

/// Minimal Effect `JsonSchema` rendering of the IR, enough for structural comparison.
pub fn render(schema: &Schema) -> Value {
    match schema {
        Schema::Ref { name } => reference(name),
        Schema::String { checks: c, .. } => {
            let mut m = Map::new();
            m.insert("type".into(), json!("string"));
            checks(&mut m, c);
            Value::Object(m)
        }
        Schema::Number { int, checks: c, .. } => {
            let mut m = Map::new();
            m.insert(
                "type".into(),
                json!(if *int { "integer" } else { "number" }),
            );
            checks(&mut m, c);
            Value::Object(m)
        }
        Schema::DateTimeMillis => json!({ "type": "number" }),
        Schema::Boolean => json!({ "type": "boolean" }),
        Schema::Null => json!({ "type": "null" }),
        Schema::Unknown => json!({}),
        Schema::Never => json!({ "not": {} }),
        Schema::Bytes => json!({ "type": "string", "format": "binary" }),
        Schema::Literal { value } => match value {
            Value::String(_) => json!({ "type": "string", "enum": [value] }),
            Value::Bool(_) => json!({ "type": "boolean", "enum": [value] }),
            _ => json!({ "type": "number", "enum": [value] }),
        },
        Schema::Array { items } => json!({ "type": "array", "items": render(items) }),
        Schema::Tuple { .. } => panic!("tuple not used by the current API"),
        Schema::Record { key, value } => match (key.as_ref(), value.as_ref()) {
            (
                Schema::String {
                    checks:
                        opencode_protocol::ir::Checks {
                            pattern: Some(p), ..
                        },
                    ..
                },
                v,
            ) => json!({ "type": "object", "patternProperties": { p.clone(): render(v) } }),
            (_, Schema::Unknown) => json!({ "type": "object" }),
            (_, v) => json!({ "type": "object", "additionalProperties": render(v) }),
        },
        Schema::Struct { fields, .. } => {
            let mut props = Map::new();
            let mut required = Vec::new();
            for f in fields {
                let mut v = render(&f.schema);
                if let (Some(d), Value::Object(m)) = (&f.description, &mut v) {
                    m.insert("description".into(), json!(d));
                }
                props.insert(f.name.clone(), v);
                if f.optionality == Optionality::Required {
                    required.push(json!(f.name));
                }
            }
            let mut m = Map::new();
            m.insert("type".into(), json!("object"));
            m.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                m.insert("required".into(), Value::Array(required));
            }
            m.insert("additionalProperties".into(), json!(false));
            Value::Object(m)
        }
        Schema::Union { members } => {
            let strings: Option<Vec<&Value>> = members
                .iter()
                .map(|m| match m {
                    Schema::Literal { value } if value.is_string() => Some(value),
                    _ => None,
                })
                .collect();
            match strings {
                Some(values) => json!({ "type": "string", "enum": values }),
                None if is_js_number(members) => json!({ "anyOf": [
                    { "type": "number" },
                    { "type": "string", "enum": ["NaN"] },
                    { "type": "string", "enum": ["Infinity"] },
                    { "type": "string", "enum": ["-Infinity"] },
                    { "type": "string", "enum": ["Infinity", "-Infinity", "NaN"] }
                ] }),
                None => json!({ "anyOf": members.iter().map(render).collect::<Vec<_>>() }),
            }
        }
        Schema::Described {
            description,
            schema,
        } => {
            let mut v = render(schema);
            if let Value::Object(m) = &mut v {
                m.insert("description".into(), json!(description));
            }
            v
        }
    }
}

pub fn is_js_number(members: &[Schema]) -> bool {
    members.len() == 4
        && matches!(members[0], Schema::Number { int: false, .. })
        && members[1..]
            .iter()
            .all(|m| matches!(m, Schema::Literal { value } if value.is_string()))
}

pub fn property_order(v: &Value) -> Vec<String> {
    v.get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn assert_schema(context: &str, ours: &Value, expected: &Value) {
    assert_eq!(ours, expected, "{context}: schema mismatch");
    assert_eq!(
        property_order(ours),
        property_order(expected),
        "{context}: field order"
    );
}

pub fn refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                out.insert(r.trim_start_matches("#/components/schemas/").to_string());
            }
            m.values().for_each(|x| refs(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

pub fn v2_operations(doc: &Value) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if op["operationId"]
                .as_str()
                .is_some_and(|id| id.starts_with("v2."))
            {
                out.push((method.to_uppercase(), path.clone(), op.clone()));
            }
        }
    }
    out
}

pub fn openapi_path(path: &str) -> String {
    path.split('/')
        .map(|s| match s.strip_prefix(':') {
            Some(p) => format!("{{{p}}}"),
            None => s.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn divergent(id: &str) -> bool {
    KNOWN_DIVERGENCES.iter().any(|(d, _)| *d == id)
}
