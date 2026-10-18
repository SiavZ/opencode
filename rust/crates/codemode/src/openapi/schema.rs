//! Purpose: OpenAPI schema projection into JSON Schema 2020-12 (port of Effect's
//! `JsonSchema.fromSchemaOpenApi3_0` / `fromSchemaOpenApi3_1` as used by codemode/openapi/spec.ts).
//! Scope: ref rewriting, OpenAPI 3.0 normalization (nullable, example, boolean exclusivity),
//! Draft-07 keyword whitelist walk, root definition extraction.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- codemode/src/openapi/spec.rs
//! Invariants: output schemas reference definitions only through `#/$defs/<name>`.
//! Contract tests: codemode/tests/openapi.rs

use serde_json::{Map, Value as Json};

const COMPONENTS: &str = "#/components/schemas";

fn replace_prefix(reference: &str, from: &str, to: &str) -> String {
    match reference.strip_prefix(from) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{to}{rest}"),
        _ => reference.to_string(),
    }
}

fn rewrite_refs(node: &Json, from: &str, to: &str) -> Json {
    match node {
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|item| rewrite_refs(item, from, to))
                .collect(),
        ),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = match (key.as_str(), value) {
                        ("$ref", Json::String(reference)) => {
                            Json::String(replace_prefix(reference, from, to))
                        }
                        (_, Json::Array(_) | Json::Object(_)) => rewrite_refs(value, from, to),
                        _ => value.clone(),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `(schema, definitions)` with root `$defs` split out (`fromSchemaDraft2020_12`).
fn split_defs(schema: Json) -> (Json, Map<String, Json>) {
    match schema {
        Json::Object(mut map) => {
            let defs = match map.shift_remove("$defs") {
                Some(Json::Object(defs)) => defs,
                _ => Map::new(),
            };
            (Json::Object(map), defs)
        }
        other => (other, Map::new()),
    }
}

/// `fromSchemaOpenApi3_1`.
pub(crate) fn from_openapi_3_1(schema: &Json) -> (Json, Map<String, Json>) {
    split_defs(rewrite_refs(schema, COMPONENTS, "#/$defs"))
}

/// `fromSchemaOpenApi3_0`.
pub(crate) fn from_openapi_3_0(schema: &Json) -> (Json, Map<String, Json>) {
    let normalized = normalize_3_0(schema);
    let mut definitions = Map::new();
    let schema = walk_draft07(&normalized, true, &mut definitions);
    (schema, definitions)
}

fn normalize_3_0(node: &Json) -> Json {
    let map = match node {
        Json::Array(items) => return Json::Array(items.iter().map(normalize_3_0).collect()),
        Json::Object(map) => map,
        other => return other.clone(),
    };
    let mut out = Map::new();
    for (key, value) in map {
        match (key.as_str(), value) {
            ("$ref", Json::String(reference)) => {
                out.insert(
                    key.clone(),
                    Json::String(replace_prefix(reference, COMPONENTS, "#/definitions")),
                );
            }
            ("example", _) => {
                if !map.contains_key("examples") {
                    out.insert("examples".into(), Json::Array(vec![value.clone()]));
                }
            }
            (_, Json::Array(_) | Json::Object(_)) => {
                out.insert(key.clone(), normalize_3_0(value));
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    for (exclusive, bound) in [
        ("exclusiveMinimum", "minimum"),
        ("exclusiveMaximum", "maximum"),
    ] {
        if let Some(Json::Bool(flag)) = out.get(exclusive).cloned() {
            match out.get(bound).cloned() {
                Some(Json::Number(limit)) if flag => {
                    out.insert(exclusive.into(), Json::Number(limit));
                    out.shift_remove(bound);
                }
                _ => {
                    out.shift_remove(exclusive);
                }
            }
        }
    }
    if out.get("nullable") == Some(&Json::Bool(true)) {
        out = apply_nullable(out);
    }
    out.shift_remove("nullable");
    Json::Object(out)
}

fn widen_type(mut node: Map<String, Json>) -> Map<String, Json> {
    match node.get("type").cloned() {
        Some(Json::String(kind)) if kind != "null" => {
            node.insert(
                "type".into(),
                Json::Array(vec![Json::String(kind), Json::String("null".into())]),
            );
        }
        Some(Json::Array(mut kinds)) if !kinds.contains(&Json::String("null".into())) => {
            kinds.push(Json::String("null".into()));
            node.insert("type".into(), Json::Array(kinds));
        }
        _ => {}
    }
    node
}

fn apply_nullable(mut node: Map<String, Json>) -> Map<String, Json> {
    if let Some(Json::Array(values)) = node.get("enum").cloned() {
        if !values.contains(&Json::Null) {
            let mut values = values;
            values.push(Json::Null);
            node.insert("enum".into(), Json::Array(values));
        }
        return widen_type(node);
    }
    if node.contains_key("type") {
        return widen_type(node);
    }
    if node.get("const") == Some(&Json::Null) {
        return node;
    }
    let mut wrapper = Map::new();
    let mut inner = node;
    inner.shift_remove("nullable");
    wrapper.insert(
        "anyOf".into(),
        Json::Array(vec![
            Json::Object(inner),
            serde_json::json!({ "type": "null" }),
        ]),
    );
    wrapper
}

const KEPT: [&str; 26] = [
    "type",
    "required",
    "enum",
    "const",
    "title",
    "description",
    "default",
    "examples",
    "format",
    "readOnly",
    "writeOnly",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
    "multipleOf",
    "uniqueItems",
    "additionalProperties",
    "propertyNames",
];

fn walk_object(value: &Json, definitions: &mut Map<String, Json>) -> Option<Json> {
    let Json::Object(map) = value else {
        return None;
    };
    Some(Json::Object(
        map.iter()
            .map(|(key, item)| (key.clone(), walk_draft07(item, false, definitions)))
            .collect(),
    ))
}

/// `fromSchemaDraft07`'s walk: keyword whitelist, ref rewrite, tuple conversion.
fn walk_draft07(node: &Json, root: bool, definitions: &mut Map<String, Json>) -> Json {
    let map = match node {
        Json::Array(items) => {
            return Json::Array(
                items
                    .iter()
                    .map(|item| walk_draft07(item, false, definitions))
                    .collect(),
            );
        }
        Json::Object(map) => map,
        other => return other.clone(),
    };
    let mut out = Map::new();
    let mut items_value: Option<&Json> = None;
    let mut additional_items: Option<&Json> = None;
    for (key, value) in map {
        match key.as_str() {
            "$ref" => {
                let rewritten = match value {
                    Json::String(reference) => {
                        Json::String(replace_prefix(reference, "#/definitions", "#/$defs"))
                    }
                    other => other.clone(),
                };
                out.insert(key.clone(), rewritten);
            }
            "definitions" => {
                let mapped = walk_object(value, definitions);
                if root {
                    if let Some(Json::Object(mapped)) = mapped {
                        *definitions = mapped;
                    }
                } else {
                    out.insert(key.clone(), mapped.unwrap_or_else(|| value.clone()));
                }
            }
            "items" => items_value = Some(value),
            "additionalItems" => additional_items = Some(value),
            "properties" | "patternProperties" => {
                out.insert(
                    key.clone(),
                    walk_object(value, definitions).unwrap_or_else(|| value.clone()),
                );
            }
            "additionalProperties" | "propertyNames" => {
                out.insert(key.clone(), walk_draft07(value, false, definitions));
            }
            "allOf" | "anyOf" | "oneOf" => {
                let walked = match value {
                    Json::Array(items) => Json::Array(
                        items
                            .iter()
                            .map(|item| walk_draft07(item, false, definitions))
                            .collect(),
                    ),
                    other => other.clone(),
                };
                out.insert(key.clone(), walked);
            }
            other if KEPT.contains(&other) => {
                out.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    match items_value {
        Some(Json::Array(prefix)) => {
            out.insert(
                "prefixItems".into(),
                Json::Array(
                    prefix
                        .iter()
                        .map(|item| walk_draft07(item, false, definitions))
                        .collect(),
                ),
            );
            if let Some(additional) = additional_items {
                out.insert("items".into(), walk_draft07(additional, false, definitions));
            }
        }
        Some(items) => {
            out.insert("items".into(), walk_draft07(items, false, definitions));
        }
        None => {}
    }
    Json::Object(out)
}
