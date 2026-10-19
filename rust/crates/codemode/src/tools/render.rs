//! Purpose: render JSON Schema (and validating schemas) as model-visible TypeScript types.
//! Scope: `tool-schema.ts` rendering: compact/pretty forms, JSDoc tags, `$ref`/`$defs`,
//! unions/intersections, recursion ceiling, input property extraction for search text.
//! Uses:
//! rust/crates/
//! |-- codemode/src/tools/mod.rs
//! |-- codemode/src/tools/text.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/openapi/responses.rs
//! |-- codemode/src/openapi/runtime.rs
//! |-- codemode/src/openapi/security.rs
//! |-- codemode/src/openapi/spec.rs
//! |-- codemode/src/output.rs
//! |-- codemode/src/stdlib/json.rs
//! |-- codemode/src/tools/discovery.rs
//! `-- codemode/src/tools/mod.rs
//! Invariants: rendering is total (never panics or errors); unresolvable shapes render `unknown`.
//! Contract tests: codemode/tests/signature.rs

use super::text::{doc_tags, jsdoc};
pub(crate) use super::text::{entries, is_identifier, json_stringify};
use super::{Definition, SchemaType};
use serde_json::{Map, Value as Json};
use std::sync::OnceLock;

const MAX_RENDER_DEPTH: usize = 8;

fn render_key(name: &str) -> String {
    if is_identifier(name) {
        name.to_string()
    } else {
        crate::value::text::quote_json(name)
    }
}

fn empty() -> &'static Map<String, Json> {
    static EMPTY: OnceLock<Map<String, Json>> = OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

/// A schema object with some keys masked (`{ ...schema, $ref: undefined }`) or `type` replaced.
#[derive(Clone, Copy)]
struct View<'a> {
    schema: &'a Map<String, Json>,
    no_ref: bool,
    no_alternatives: bool,
    no_all_of: bool,
    type_override: Option<&'a Json>,
}

impl<'a> View<'a> {
    fn of(schema: &'a Json) -> Self {
        Self {
            schema: schema.as_object().unwrap_or_else(|| empty()),
            no_ref: false,
            no_alternatives: false,
            no_all_of: false,
            type_override: None,
        }
    }

    fn get(&self, key: &str) -> Option<&'a Json> {
        match key {
            "$ref" if self.no_ref => None,
            "anyOf" | "oneOf" if self.no_alternatives => None,
            "allOf" if self.no_all_of => None,
            "type" if self.type_override.is_some() => self.type_override,
            _ => self.schema.get(key),
        }
    }
}

#[derive(Clone)]
struct Ctx<'a> {
    definitions: Vec<&'a Map<String, Json>>,
    pretty: bool,
}

impl<'a> Ctx<'a> {
    fn lookup(&self, name: &str) -> Option<&'a Json> {
        self.definitions.iter().rev().find_map(|map| map.get(name))
    }
}

fn ref_name(reference: &str) -> Option<String> {
    let rest = reference
        .strip_prefix("#/$defs/")
        .or_else(|| reference.strip_prefix("#/definitions/"))?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.replace("~1", "/").replace("~0", "~"))
}

fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(flag) => *flag,
        Json::Number(num) => num.as_f64().is_some_and(|value| value != 0.0),
        Json::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn intersection(members: Vec<String>) -> String {
    let concrete: Vec<String> = members
        .into_iter()
        .filter(|member| member != "unknown")
        .collect();
    match concrete.len() {
        0 => "unknown".into(),
        1 => concrete
            .into_iter()
            .next()
            .unwrap_or_else(|| "unknown".into()),
        _ => concrete
            .iter()
            .map(|member| {
                if member.contains(" | ") {
                    format!("({member})")
                } else {
                    member.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" & "),
    }
}

fn has_unresolved_ref(
    schema: &Json,
    ctx: &Ctx<'_>,
    seen: &[String],
    visited: &[*const Json],
) -> bool {
    let pointer = schema as *const Json;
    if visited.contains(&pointer) {
        return false;
    }
    let mut next_visited = visited.to_vec();
    next_visited.push(pointer);
    let view = View::of(schema);
    if let Some(reference) = view.get("$ref") {
        let Some(name) = reference.as_str().and_then(ref_name) else {
            return true;
        };
        let Some(target) = ctx.lookup(&name) else {
            return true;
        };
        if seen.contains(&name) {
            return true;
        }
        let mut next_seen = seen.to_vec();
        next_seen.push(name);
        if has_unresolved_ref(target, ctx, &next_seen, &next_visited) {
            return true;
        }
    }
    let mut children: Vec<&Json> = Vec::new();
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Json::Array(items)) = view.get(key) {
            children.extend(items);
        }
    }
    if let Some(Json::Object(properties)) = view.get("properties") {
        children.extend(entries(properties).into_iter().map(|(_, item)| item));
    }
    if let Some(items) = view.get("items") {
        children.push(items);
    }
    if let Some(additional @ (Json::Object(_) | Json::Array(_))) = view.get("additionalProperties")
    {
        children.push(additional);
    }
    children
        .into_iter()
        .any(|item| has_unresolved_ref(item, ctx, seen, &next_visited))
}

fn is_number_sentinel(item: &Json) -> bool {
    let view = View::of(item);
    view.get("type") == Some(&Json::String("string".into()))
        && matches!(view.get("enum"), Some(Json::Array(values)) if values.len() == 1
            && matches!(values[0].as_str(), Some("NaN" | "Infinity" | "-Infinity")))
}

fn render(view: View<'_>, ctx: &Ctx<'_>, depth: usize, seen: &[String]) -> String {
    if depth > MAX_RENDER_DEPTH {
        return "unknown".into();
    }
    let mut nested = ctx.clone();
    for key in ["definitions", "$defs"] {
        if let Some(Json::Object(map)) = view.get(key) {
            nested.definitions.push(map);
        }
    }
    if let Some(reference) = view.get("$ref").filter(|value| truthy(value)) {
        let Some(name) = reference.as_str().and_then(ref_name) else {
            return "unknown".into();
        };
        let Some(target) = nested.lookup(&name).filter(|value| truthy(value)) else {
            return "unknown".into();
        };
        if seen.contains(&name) {
            return "unknown".into();
        }
        let mut next_seen = seen.to_vec();
        next_seen.push(name);
        let resolved = render(View::of(target), &nested, depth, &next_seen);
        let sibling = render(
            View {
                no_ref: true,
                ..view
            },
            &nested,
            depth + 1,
            seen,
        );
        return intersection(vec![resolved, sibling]);
    }
    if let Some(constant) = view.get("const") {
        return json_stringify(constant);
    }
    if let Some(values) = view.get("enum").filter(|value| truthy(value)) {
        return match values {
            Json::Array(values) => values
                .iter()
                .map(json_stringify)
                .collect::<Vec<_>>()
                .join(" | "),
            _ => "unknown".into(),
        };
    }
    if let Some(alternatives) = view.get("anyOf").or_else(|| view.get("oneOf")) {
        let Json::Array(items) = alternatives else {
            return "unknown".into();
        };
        let item_type = |item: &Json| View::of(item).get("type").cloned();
        if items
            .iter()
            .any(|item| item_type(item) == Some(Json::String("number".into())))
            && items.iter().all(|item| {
                item_type(item) == Some(Json::String("number".into())) || is_number_sentinel(item)
            })
        {
            return "number".into();
        }
        if items.len() == 2
            && item_type(&items[0]) == Some(Json::String("object".into()))
            && View::of(&items[0]).get("properties").is_none()
            && item_type(&items[1]) == Some(Json::String("array".into()))
            && View::of(&items[1]).get("items").is_none()
        {
            return "{}".into();
        }
        let members: Vec<String> = items
            .iter()
            .map(|item| render(View::of(item), &nested, depth + 1, seen))
            .collect();
        if members.iter().any(|member| member == "unknown") {
            return "unknown".into();
        }
        let rest = render(
            View {
                no_alternatives: true,
                ..view
            },
            &nested,
            depth + 1,
            seen,
        );
        return intersection(vec![members.join(" | "), rest]);
    }
    if let Some(all_of) = view.get("allOf") {
        let Json::Array(items) = all_of else {
            return "unknown".into();
        };
        let members: Vec<String> = items
            .iter()
            .map(|item| render(View::of(item), &nested, depth + 1, seen))
            .collect();
        if items
            .iter()
            .any(|item| has_unresolved_ref(item, &nested, &[], &[]))
        {
            return "unknown".into();
        }
        let mut all = vec![render(
            View {
                no_all_of: true,
                ..view
            },
            &nested,
            depth + 1,
            seen,
        )];
        all.extend(members);
        return intersection(all);
    }
    match view.get("type") {
        Some(Json::Array(types)) => {
            return types
                .iter()
                .map(|item| {
                    render(
                        View {
                            type_override: Some(item),
                            ..view
                        },
                        &nested,
                        depth + 1,
                        seen,
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
        }
        Some(Json::String(kind)) => match kind.as_str() {
            "string" => return "string".into(),
            "number" | "integer" => return "number".into(),
            "boolean" => return "boolean".into(),
            "null" => return "null".into(),
            "array" => {
                let items = view
                    .get("items")
                    .map_or_else(|| View::of(&Json::Null), View::of);
                return format!("Array<{}>", render(items, &nested, depth + 1, seen));
            }
            _ => {}
        },
        _ => {}
    }
    let is_object = view.get("type") == Some(&Json::String("object".into()));
    if is_object || view.get("properties").is_some_and(truthy) {
        return render_object(view, &nested, depth, seen);
    }
    "unknown".into()
}

fn render_object(view: View<'_>, ctx: &Ctx<'_>, depth: usize, seen: &[String]) -> String {
    let required: Vec<&str> = match view.get("required") {
        Some(Json::Array(items)) => items.iter().filter_map(Json::as_str).collect(),
        _ => Vec::new(),
    };
    let properties = match view.get("properties") {
        Some(Json::Object(map)) => entries(map),
        _ => Vec::new(),
    };
    let index_type = match view.get("additionalProperties") {
        Some(additional @ (Json::Object(_) | Json::Array(_))) => {
            Some(render(View::of(additional), ctx, depth + 1, seen))
        }
        _ => None,
    };
    let field = |name: &str, value: &Json| {
        let optional = if required.contains(&name) { "" } else { "?" };
        format!(
            "{}{optional}: {}",
            render_key(name),
            render(View::of(value), ctx, depth + 1, seen)
        )
    };
    if !ctx.pretty {
        let mut fields: Vec<String> = properties
            .iter()
            .map(|(name, value)| field(name, value))
            .collect();
        if let Some(index) = &index_type {
            fields.push(format!("[key: string]: {index}"));
        }
        return if fields.is_empty() {
            "{}".into()
        } else {
            format!("{{ {} }}", fields.join("; "))
        };
    }
    if properties.is_empty() && index_type.is_none() {
        return "{}".into();
    }
    let pad = "  ".repeat(depth + 1);
    let mut lines: Vec<String> = properties
        .iter()
        .map(|(name, value)| {
            let description = View::of(value).get("description").and_then(Json::as_str);
            format!(
                "{}{pad}{},",
                jsdoc(description, &doc_tags(value), &pad),
                field(name, value)
            )
        })
        .collect();
    if let Some(index) = &index_type {
        lines.push(format!("{pad}[key: string]: {index},"));
    }
    format!("{{\n{}\n{}}}", lines.join("\n"), "  ".repeat(depth))
}

fn root_definitions(schema: &Json) -> Vec<&Map<String, Json>> {
    ["definitions", "$defs"]
        .into_iter()
        .filter_map(|key| schema.get(key).and_then(Json::as_object))
        .collect()
}

/// Renders a raw JSON Schema document as a TypeScript type string.
pub fn json_schema_to_type_script(schema: &Json, pretty: bool) -> String {
    render(
        View::of(schema),
        &Ctx {
            definitions: root_definitions(schema),
            pretty,
        },
        0,
        &[],
    )
}

fn schema_type_script(schema: &SchemaType, decoded: bool, pretty: bool) -> String {
    match schema {
        SchemaType::Validating(schema) => {
            let document = schema.json_schema(decoded);
            render(
                View::of(&document),
                &Ctx {
                    definitions: Vec::new(),
                    pretty,
                },
                0,
                &[],
            )
        }
        SchemaType::Json(schema) => json_schema_to_type_script(schema, pretty),
    }
}

/// The model-visible TypeScript type of a tool's input.
pub fn input_type_script(definition: &Definition, pretty: bool) -> String {
    schema_type_script(&definition.input, false, pretty)
}

/// The model-visible TypeScript type of a tool's result (`unknown` without an output schema).
pub fn output_type_script(definition: &Definition, pretty: bool) -> String {
    definition.output.as_ref().map_or_else(
        || "unknown".into(),
        |output| schema_type_script(output, true, pretty),
    )
}

/// Input property names and descriptions, best-effort, for search text.
pub(crate) fn input_properties(definition: &Definition) -> Vec<(String, Option<String>)> {
    let document = match &definition.input {
        SchemaType::Validating(schema) => schema.json_schema(false),
        SchemaType::Json(schema) => schema.clone(),
    };
    let definitions = root_definitions(&document);
    let mut schema = &document;
    if let Some(reference) = document.get("$ref") {
        let resolved = reference
            .as_str()
            .and_then(ref_name)
            .and_then(|name| definitions.iter().rev().find_map(|map| map.get(&name)));
        match resolved {
            Some(resolved) => schema = resolved,
            None => return Vec::new(),
        }
    }
    match schema.get("properties") {
        Some(Json::Object(properties)) => entries(properties)
            .into_iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value
                        .get("description")
                        .and_then(Json::as_str)
                        .map(str::to_string),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}
