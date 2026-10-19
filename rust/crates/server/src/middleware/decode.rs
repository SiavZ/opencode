//! Purpose: decode and validate request params, query and payload against the protocol schema IR,
//! producing the baseline schema-rejection errors (kind `Params`, `Query`, `Payload`).
//! Scope: per-request pure decoding.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! `-- protocol/src/lib.rs
//! External:
//! |-- f64::NAN
//! |-- form_urlencoded::parse
//! |-- serde_json::Map
//! |-- serde_json::Number::from_f64
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/streams.rs
//! `-- server/src/middleware/mod.rs
//! Invariants: unknown query keys and payload keys are ignored (Effect `onExcessProperty`
//! default). Bracketed query keys (`location[directory]`) build nested objects for struct-typed
//! query fields. `NumberFromString` fields decode to JSON numbers before checks run. An empty
//! query value counts as present (matches URLSearchParams).
//! Contract tests: server/tests/http_errors.rs

use opencode_protocol::Api;
use opencode_protocol::ir::{Field, Optionality, Schema};
use serde_json::{Map, Value};

/// Build the decoded query object for `fields` from a raw query string.
pub fn query(api: &Api, fields: &[Field], raw: Option<&str>) -> Result<Map<String, Value>, String> {
    let pairs: Vec<(String, String)> = raw
        .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    let mut out = Map::new();
    for field in fields {
        let schema = resolve(api, &field.schema);
        if let Schema::Struct { fields: inner, .. } = schema {
            let mut nested = Map::new();
            for sub in inner {
                let key = format!("{}[{}]", field.name, sub.name);
                if let Some((_, value)) = pairs.iter().find(|(k, _)| *k == key) {
                    nested.insert(sub.name.clone(), Value::String(value.clone()));
                }
            }
            if !nested.is_empty() {
                out.insert(field.name.clone(), Value::Object(nested));
            }
            continue;
        }
        let values: Vec<&String> = pairs
            .iter()
            .filter(|(k, _)| *k == field.name)
            .map(|(_, v)| v)
            .collect();
        let Some(first) = values.first() else {
            continue;
        };
        let value = match schema {
            Schema::Array { .. } => {
                Value::Array(values.iter().map(|v| Value::String((*v).clone())).collect())
            }
            _ => Value::String((*first).clone()),
        };
        out.insert(field.name.clone(), value);
    }
    let value = Value::Object(out);
    let decoded = decode_fields(api, fields, &value, "")?;
    Ok(decoded)
}

/// Decode path params into an object and validate them.
pub fn params(
    api: &Api,
    fields: &[Field],
    raw: &[(String, String)],
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for field in fields {
        if let Some((_, v)) = raw.iter().find(|(k, _)| *k == field.name) {
            out.insert(field.name.clone(), Value::String(v.clone()));
        }
    }
    decode_fields(api, fields, &Value::Object(out), "")
}

/// Validate a JSON payload, returning it unchanged on success.
pub fn payload(api: &Api, schema: &Schema, value: &Value) -> Result<(), String> {
    validate(api, schema, value, "").map(|_| ())
}

fn decode_fields(
    api: &Api,
    fields: &[Field],
    value: &Value,
    path: &str,
) -> Result<Map<String, Value>, String> {
    let schema = Schema::Struct {
        fields: fields.to_vec(),
        index: None,
    };
    match validate(api, &schema, value, path)? {
        Value::Object(map) => Ok(map),
        _ => Ok(Map::new()),
    }
}

fn resolve<'a>(api: &'a Api, schema: &'a Schema) -> &'a Schema {
    match schema {
        Schema::Ref { name } => api
            .definition(name)
            .map(|d| resolve(api, &d.schema))
            .unwrap_or(schema),
        Schema::Described { schema, .. } => resolve(api, schema),
        _ => schema,
    }
}

fn at(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("\n  at {path}")
    }
}

/// Validate (and coerce `from_string` numbers), returning the decoded value.
fn validate(api: &Api, schema: &Schema, value: &Value, path: &str) -> Result<Value, String> {
    let expected = |what: &str| {
        let label = match schema {
            Schema::Ref { name } if what == "object" => name.as_str(),
            _ => what,
        };
        Err(format!(
            "Expected {label}, got {}{}",
            short(value),
            at(path)
        ))
    };
    match resolve(api, schema) {
        Schema::Ref { name } => Err(format!("Unresolved schema {name}{}", at(path))),
        Schema::Unknown => Ok(value.clone()),
        Schema::Never => expected("never"),
        Schema::Null => {
            if value.is_null() {
                Ok(Value::Null)
            } else {
                expected("null")
            }
        }
        Schema::Boolean => {
            if value.is_boolean() {
                Ok(value.clone())
            } else {
                expected("boolean")
            }
        }
        Schema::Literal { value: lit } => {
            if value == lit {
                Ok(value.clone())
            } else {
                expected(&lit.to_string())
            }
        }
        Schema::String { checks, .. } => {
            let Some(s) = value.as_str() else {
                return expected("string");
            };
            if let Some(pattern) = &checks.pattern
                && let Some(prefix) = pattern.strip_prefix('^')
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !s.starts_with(prefix)
            {
                return Err(format!(
                    "Expected a string matching the pattern {pattern}, got {}{}",
                    short(value),
                    at(path)
                ));
            }
            if checks
                .min_length
                .is_some_and(|m| (s.chars().count() as u64) < m)
            {
                return expected("a longer string");
            }
            if checks
                .max_length
                .is_some_and(|m| (s.chars().count() as u64) > m)
            {
                return expected("a shorter string");
            }
            Ok(value.clone())
        }
        Schema::Number {
            int,
            checks,
            from_string,
        } => {
            let number = match (value, from_string) {
                (Value::String(s), true) => match s.trim().parse::<f64>() {
                    Ok(n) if !s.trim().is_empty() => n,
                    _ => {
                        return Err(format!(
                            "Expected a string representing a finite number, got {}{}",
                            short(value),
                            at(path)
                        ));
                    }
                },
                (Value::Number(n), false) => n.as_f64().unwrap_or(f64::NAN),
                _ => return expected("number"),
            };
            if *int && number.fract() != 0.0 {
                return Err(format!("Expected an integer, got {number}{}", at(path)));
            }
            if checks.minimum.is_some_and(|m| number < m)
                || checks.exclusive_minimum.is_some_and(|m| number <= m)
            {
                return Err(format!(
                    "Expected a value greater than the minimum, got {number}{}",
                    at(path)
                ));
            }
            if checks.maximum.is_some_and(|m| number > m)
                || checks.exclusive_maximum.is_some_and(|m| number >= m)
            {
                return Err(format!(
                    "Expected a value less than the maximum, got {number}{}",
                    at(path)
                ));
            }
            if *from_string {
                return Ok(serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .unwrap_or(Value::Null));
            }
            Ok(value.clone())
        }
        Schema::DateTimeMillis => {
            if value.is_number() {
                Ok(value.clone())
            } else {
                expected("number")
            }
        }
        Schema::Bytes => Ok(value.clone()),
        Schema::Array { items } => {
            let Some(list) = value.as_array() else {
                return expected("array");
            };
            let mut out = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                out.push(validate(api, items, item, &format!("{path}[{i}]"))?);
            }
            Ok(Value::Array(out))
        }
        Schema::Tuple { elements, rest } => {
            let Some(list) = value.as_array() else {
                return expected("array");
            };
            let mut out = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let schema = match elements.get(i) {
                    Some(s) => s,
                    None => match rest {
                        Some(r) => r,
                        None => return expected("a shorter tuple"),
                    },
                };
                out.push(validate(api, schema, item, &format!("{path}[{i}]"))?);
            }
            if list.len() < elements.len() {
                return Err(format!(
                    "Missing key{}",
                    at(&format!("{path}[{}]", list.len()))
                ));
            }
            Ok(Value::Array(out))
        }
        Schema::Record { value: inner, .. } => {
            let Some(map) = value.as_object() else {
                return expected("object");
            };
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(
                    k.clone(),
                    validate(api, inner, v, &format!("{path}[\"{k}\"]"))?,
                );
            }
            Ok(Value::Object(out))
        }
        Schema::Struct { fields, index } => {
            let Some(map) = value.as_object() else {
                return expected("object");
            };
            let mut out = Map::new();
            for field in fields {
                let child = format!("{path}[\"{}\"]", field.name);
                match map.get(&field.name) {
                    None if field.optionality != Optionality::Required => {}
                    None => return Err(format!("Missing key{}", at(&child))),
                    Some(v) => {
                        out.insert(field.name.clone(), validate(api, &field.schema, v, &child)?);
                    }
                }
            }
            if let Some((_, inner)) = index {
                for (k, v) in map {
                    if !fields.iter().any(|f| f.name == *k) {
                        out.insert(
                            k.clone(),
                            validate(api, inner, v, &format!("{path}[\"{k}\"]"))?,
                        );
                    }
                }
            }
            Ok(Value::Object(out))
        }
        Schema::Union { members } => {
            if matches!(schema, Schema::Ref { .. })
                && !value.is_object()
                && members.iter().all(|member| {
                    matches!(
                        resolve(api, member),
                        Schema::Struct { .. } | Schema::Record { .. }
                    )
                })
            {
                return expected("object");
            }
            let mut last = None;
            for member in members {
                match validate(api, member, value, path) {
                    Ok(v) => return Ok(v),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| format!("Expected never{}", at(path))))
        }
        Schema::Described { .. } => Ok(value.clone()),
    }
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 80 {
        format!("{}...", text.chars().take(80).collect::<String>())
    } else {
        text
    }
}
