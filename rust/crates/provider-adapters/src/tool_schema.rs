//! Purpose: tool JSON-schema projections shared by protocol families: OpenAI flat-object,
//! Gemini dialect, Moonshot tuple/$ref rules and model-compatibility dispatch
//! (protocols/utils/tool-schema.ts, gemini-tool-schema.ts).
//! Scope: pure JSON transforms.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: key order follows the baseline object construction order. Inputs are never mutated.
//! Contract tests: provider-adapters/src/tool_schema.rs (unit tests, cases from packages/llm/test/tool-schema-projection.test.ts)

use opencode_llm_api::{JsonObject, ToolSchemaCompatibility};
use serde_json::{Map, Value, json};

/// Apply model compatibility before protocol projection.
pub fn model_compatibility(
    schema: &JsonObject,
    compatibility: Option<ToolSchemaCompatibility>,
) -> JsonObject {
    match compatibility {
        None => schema.clone(),
        Some(ToolSchemaCompatibility::Gemini) => gemini(schema),
        Some(ToolSchemaCompatibility::Moonshot) => moonshot(schema),
    }
}

fn remove_null_schemas(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(remove_null_schemas).collect()),
        Value::Object(map) => {
            let mut fields: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| *k != "anyOf")
                .map(|(k, v)| (k.clone(), remove_null_schemas(v)))
                .collect();
            let Some(Value::Array(any_of)) = map.get("anyOf") else {
                return Value::Object(fields);
            };
            let variants: Vec<Value> = any_of
                .iter()
                .filter(|v| !(v.is_object() && v.get("type") == Some(&json!("null"))))
                .map(remove_null_schemas)
                .collect();
            if let [Value::Object(only)] = variants.as_slice() {
                for (k, v) in only {
                    fields.insert(k.clone(), v.clone());
                }
                return Value::Object(fields);
            }
            fields.insert("anyOf".into(), Value::Array(variants));
            Value::Object(fields)
        }
        other => other.clone(),
    }
}

/// `ToolSchemaProjection.openAI`: one flat object at top level, nullable unions collapsed.
pub fn openai(schema: &JsonObject) -> JsonObject {
    let variants: Vec<&Map<String, Value>> = match schema.get("anyOf") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).collect(),
        _ => vec![],
    };
    let mut flattened = Map::new();
    if variants.is_empty() {
        flattened = schema.clone();
        flattened.insert("type".into(), json!("object"));
    } else {
        for (k, v) in schema.iter().filter(|(k, _)| *k != "anyOf") {
            flattened.insert(k.clone(), v.clone());
        }
        flattened.insert("type".into(), json!("object"));
        // `reduce((acc, v) => ({ ...v.properties, ...acc }))`: earlier variants win on value,
        // key positions come from the spread order (current variant first, then accumulated).
        let mut properties = Map::new();
        for variant in &variants {
            let mut next = Map::new();
            if let Some(Value::Object(props)) = variant.get("properties") {
                for (k, v) in props {
                    next.insert(k.clone(), v.clone());
                }
            }
            for (k, v) in &properties {
                next.insert(k.clone(), v.clone());
            }
            properties = next;
        }
        flattened.insert("properties".into(), Value::Object(properties));
        flattened.insert("additionalProperties".into(), json!(false));
    }
    match remove_null_schemas(&Value::Object(flattened)) {
        Value::Object(map) => map,
        _ => json!({ "type": "object" })
            .as_object()
            .cloned()
            .unwrap_or_default(),
    }
}

fn tuple_items_schema(items: &[Value]) -> Value {
    let projected: Vec<Value> = items.iter().map(moonshot_node).collect();
    match projected.len() {
        0 => json!({}),
        1 => projected[0].clone(),
        _ => json!({ "anyOf": projected }),
    }
}

fn moonshot_node(schema: &Value) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.iter().map(moonshot_node).collect()),
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                return json!({ "$ref": r });
            }
            let mut out = Map::new();
            for (key, value) in map {
                match key.as_str() {
                    "items" if value.is_array() => {
                        out.insert(
                            key.clone(),
                            tuple_items_schema(value.as_array().map(Vec::as_slice).unwrap_or(&[])),
                        );
                    }
                    "prefixItems" => {
                        if map.contains_key("items") {
                            continue;
                        }
                        out.insert(
                            "items".into(),
                            tuple_items_schema(value.as_array().map(Vec::as_slice).unwrap_or(&[])),
                        );
                    }
                    "unevaluatedItems" => {}
                    _ => {
                        out.insert(key.clone(), moonshot_node(value));
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// `ToolSchemaProjection.moonshot`.
pub fn moonshot(schema: &JsonObject) -> JsonObject {
    match moonshot_node(&Value::Object(schema.clone())) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

const SCHEMA_INTENT_KEYS: &[&str] = &[
    "type",
    "properties",
    "items",
    "prefixItems",
    "enum",
    "const",
    "$ref",
    "additionalProperties",
    "patternProperties",
    "required",
    "not",
    "if",
    "then",
    "else",
];

fn has_combiner(schema: &Map<String, Value>) -> bool {
    ["anyOf", "oneOf", "allOf"]
        .iter()
        .any(|k| schema.get(*k).is_some_and(Value::is_array))
}

fn has_schema_intent(schema: &Map<String, Value>) -> bool {
    has_combiner(schema) || SCHEMA_INTENT_KEYS.iter().any(|k| schema.contains_key(*k))
}

fn js_string(value: &Value) -> Value {
    Value::String(crate::shared::js_string(value))
}

fn sanitize_node(schema: &Value) -> Value {
    let map = match schema {
        Value::Object(map) => map,
        Value::Array(items) => return Value::Array(items.iter().map(sanitize_node).collect()),
        other => return other.clone(),
    };
    let mut result: Map<String, Value> = map
        .iter()
        .map(|(k, v)| {
            let value = match (k.as_str(), v) {
                ("enum", Value::Array(items)) => {
                    Value::Array(items.iter().map(js_string).collect())
                }
                _ => sanitize_node(v),
            };
            (k.clone(), value)
        })
        .collect();
    let ty = result.get("type").cloned();
    if result.get("enum").is_some_and(Value::is_array)
        && (ty == Some(json!("integer")) || ty == Some(json!("number")))
    {
        result.insert("type".into(), json!("string"));
    }
    if result.get("type") == Some(&json!("object"))
        && let (Some(Value::Object(props)), Some(Value::Array(required))) =
            (result.get("properties"), result.get("required"))
    {
        let filtered: Vec<Value> = required
            .iter()
            .filter(|f| f.as_str().is_some_and(|s| props.contains_key(s)))
            .cloned()
            .collect();
        result.insert("required".into(), Value::Array(filtered));
    }
    if result.get("type") == Some(&json!("array")) && !has_combiner(&result) {
        let items = result.get("items").cloned().unwrap_or_else(|| json!({}));
        let items = match items {
            Value::Object(mut obj) if !has_schema_intent(&obj) => {
                obj.insert("type".into(), json!("string"));
                Value::Object(obj)
            }
            other => other,
        };
        result.insert("items".into(), items);
    }
    if matches!(result.get("type"), Some(Value::String(t)) if t != "object")
        && !has_combiner(&result)
    {
        result.shift_remove("properties");
        result.shift_remove("required");
    }
    Value::Object(result)
}

fn empty_object_schema(schema: &Map<String, Value>) -> bool {
    schema.get("type") == Some(&json!("object"))
        && schema
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(Map::is_empty)
        && !schema.get("additionalProperties").is_some_and(truthy)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn project_list(value: Option<&Value>) -> Option<Value> {
    value
        .and_then(Value::as_array)
        .map(|items| Value::Array(items.iter().map(project_or_null).collect()))
}

/// `projectNode` inside arrays and records: `undefined` serializes as `null` in arrays and is
/// dropped in records.
fn project_or_null(value: &Value) -> Value {
    project_node(value)
        .map(Value::Object)
        .unwrap_or(Value::Null)
}

fn project_node(schema: &Value) -> Option<Map<String, Value>> {
    let schema = schema.as_object()?;
    if empty_object_schema(schema) {
        return None;
    }
    let mut out = Map::new();
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            out.insert(k.into(), v);
        }
    };
    put("description", schema.get("description").cloned());
    put("required", schema.get("required").cloned());
    put("format", schema.get("format").cloned());
    let ty = match schema.get("type") {
        Some(Value::Array(types)) => types.iter().find(|t| *t != &json!("null")).cloned(),
        other => other.cloned(),
    };
    put("type", ty);
    let nullable =
        matches!(schema.get("type"), Some(Value::Array(t)) if t.contains(&json!("null")));
    put("nullable", nullable.then_some(json!(true)));
    put(
        "enum",
        schema
            .get("const")
            .map(|c| json!([c]))
            .or_else(|| schema.get("enum").cloned()),
    );
    put(
        "properties",
        schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| {
                Value::Object(
                    props
                        .iter()
                        .filter_map(|(k, v)| project_node(v).map(|p| (k.clone(), Value::Object(p))))
                        .collect(),
                )
            }),
    );
    put(
        "items",
        match schema.get("items") {
            Some(Value::Array(items)) => {
                Some(Value::Array(items.iter().map(project_or_null).collect()))
            }
            None => None,
            Some(item) => project_node(item).map(Value::Object),
        },
    );
    put("allOf", project_list(schema.get("allOf")));
    put("anyOf", project_list(schema.get("anyOf")));
    put("oneOf", project_list(schema.get("oneOf")));
    put("minLength", schema.get("minLength").cloned());
    Some(out)
}

/// `GeminiToolSchema.convert` (empty object when the projection is `undefined`).
pub fn gemini(schema: &JsonObject) -> JsonObject {
    project_node(&sanitize_node(&Value::Object(schema.clone()))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> JsonObject {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn moonshot_strips_ref_siblings_and_tuples() {
        let out = moonshot(&obj(json!({
            "type": "object",
            "properties": {
                "linked": { "$ref": "#/$defs/Linked", "description": "drop me" },
                "tuple": { "type": "array", "items": [{ "type": "string" }, { "type": "number" }] },
                "prefixTuple": { "type": "array", "prefixItems": [{ "type": "boolean" }, { "type": "string" }] }
            }
        })));
        assert_eq!(
            Value::Object(out),
            json!({
                "type": "object",
                "properties": {
                    "linked": { "$ref": "#/$defs/Linked" },
                    "tuple": { "type": "array", "items": { "anyOf": [{ "type": "string" }, { "type": "number" }] } },
                    "prefixTuple": { "type": "array", "items": { "anyOf": [{ "type": "boolean" }, { "type": "string" }] } }
                }
            })
        );
    }

    #[test]
    fn gemini_enums_required_arrays_scalars() {
        let out = gemini(&obj(json!({
            "type": "object",
            "required": ["status", "missing"],
            "properties": {
                "status": { "type": "integer", "enum": [1, 2] },
                "tags": { "type": "array" },
                "name": { "type": "string", "properties": { "ignored": { "type": "string" } }, "required": ["ignored"] }
            }
        })));
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            serde_json::to_string(&json!({
                "required": ["status"],
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["1", "2"] },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "name": { "type": "string" }
                }
            }))
            .unwrap()
        );
    }

    #[test]
    fn openai_flattens_any_of() {
        let out = openai(&obj(json!({
            "anyOf": [
                { "type": "object", "properties": { "path": { "type": "string" }, "maybe": { "anyOf": [{ "type": "string" }, { "type": "null" }] } } },
                { "type": "object", "properties": { "resource": { "type": "string" } } }
            ]
        })));
        assert_eq!(
            Value::Object(out),
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "maybe": { "type": "string" }, "resource": { "type": "string" } },
                "additionalProperties": false
            })
        );
    }
}
