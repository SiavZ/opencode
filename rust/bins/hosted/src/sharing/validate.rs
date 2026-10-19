//! Purpose: zod 4.1 issue shapes for the enterprise API validators
//! (packages/enterprise/src/routes/api/[...path].ts) so 400 bodies match the baseline byte for byte.
//! Scope: pure validation of decoded JSON, only the schemas the share routes use.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! |-- hosted/src/sharing/enterprise.rs
//! `-- hosted/src/sharing/mod.rs
//! Invariants: issue key order and messages are the captured zod output
//! (crates/hosted-adapters/tests/fixtures/sharing/http.json). Object checks report every field
//! issue in schema order, array checks report every element issue in index order.
//! Contract tests: bins/hosted/src/sharing/tests.rs

use serde_json::{Value, json};

pub const DATA_TYPES: [&str; 5] = ["session", "message", "part", "session_diff", "model"];

/// zod's received-type name.
fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// `invalid_type` issue in the key order zod emits for object fields.
fn field_issue(expected: &str, path: Value, value: Option<&Value>) -> Value {
    json!({
        "expected": expected,
        "code": "invalid_type",
        "path": path,
        "message": format!("Invalid input: expected {expected}, received {}", received(value)),
    })
}

/// `invalid_type` issue for an element inside a discriminated union array (code first).
fn element_issue(path: Value, value: Option<&Value>) -> Value {
    json!({
        "code": "invalid_type",
        "expected": "object",
        "path": path,
        "message": format!("Invalid input: expected object, received {}", received(value)),
    })
}

fn root_object(body: &Value) -> Result<&serde_json::Map<String, Value>, Vec<Value>> {
    body.as_object()
        .ok_or_else(|| vec![field_issue("object", json!([]), Some(body))])
}

/// `z.object({...string fields})`.
pub fn strings(body: &Value, fields: &[&str]) -> Result<(), Vec<Value>> {
    let object = root_object(body)?;
    let issues: Vec<Value> = fields
        .iter()
        .filter(|f| !object.get(**f).is_some_and(Value::is_string))
        .map(|f| field_issue("string", json!([f]), object.get(*f)))
        .collect();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// `z.object({ secret: z.string(), data: Share.Data.array() })`.
pub fn sync_body(body: &Value) -> Result<(), Vec<Value>> {
    let object = root_object(body)?;
    let mut issues = Vec::new();
    if !object.get("secret").is_some_and(Value::is_string) {
        issues.push(field_issue(
            "string",
            json!(["secret"]),
            object.get("secret"),
        ));
    }
    match object.get("data") {
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                match item {
                    Value::Object(map) => {
                        let ok = map
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|t| DATA_TYPES.contains(&t));
                        if !ok {
                            issues.push(json!({
                                "code": "invalid_union",
                                "errors": [],
                                "note": "No matching discriminator",
                                "discriminator": "type",
                                "path": ["data", index, "type"],
                                "message": "Invalid input",
                            }));
                        }
                    }
                    other => issues.push(element_issue(json!(["data", index]), Some(other))),
                }
            }
        }
        other => issues.push(field_issue("array", json!(["data"]), other)),
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// `z.object({ shareID: z.string().min(1) })` used by the support route via safeParse.
pub fn support_body(body: Option<&Value>) -> Result<String, Vec<Value>> {
    let Some(body) = body else {
        return Err(vec![field_issue("object", json!([]), None)]);
    };
    let object = root_object(body)?;
    match object.get("shareID") {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(vec![json!({
            "origin": "string",
            "code": "too_small",
            "minimum": 1,
            "inclusive": true,
            "path": ["shareID"],
            "message": "Too small: expected string to have >=1 characters",
        })]),
        other => Err(vec![field_issue("string", json!(["shareID"]), other)]),
    }
}

/// hono-openapi validator failure body: `{data, error, success:false}`.
pub fn failure(body: &Value, issues: Vec<Value>) -> Value {
    json!({ "data": body, "error": issues, "success": false })
}
