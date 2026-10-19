//! Purpose: OpenAPI 3.x document analysis: refs, parameters, request bodies, responses, operation
//! paths, servers, and security requirements/schemes.
//! Scope: codemode/src/openapi/spec.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/openapi/schema.rs
//! |-- codemode/src/tools/render.rs
//! `-- codemode/src/value/convert.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/openapi/naming.rs
//! |-- codemode/src/openapi/responses.rs
//! `-- codemode/src/openapi/security.rs
//! Invariants: lookups keyed by spec- or model-controlled names never read inherited values;
//! unrepresentable operations produce a reason instead of a tool.
//! Contract tests: codemode/tests/openapi.rs

use super::schema::{from_openapi_3_0, from_openapi_3_1};
use super::{Body, BodyMode, InputField, Location, Style};
use crate::tools::render::entries;
use crate::value::convert::is_blocked_member;
use serde_json::{Map, Value as Json, json};

pub(crate) const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];
const IGNORED_HEADERS: [&str; 3] = ["accept", "content-type", "authorization"];

pub(crate) type Parsed<T> = Result<T, String>;

pub(crate) fn non_empty(value: Option<&Json>) -> Option<String> {
    value
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub(crate) fn array(value: Option<&Json>) -> Vec<Json> {
    value.and_then(Json::as_array).cloned().unwrap_or_default()
}

/// Follows local `#/...` refs (with `~0`/`~1` unescaping) until a non-ref or a cycle.
pub(crate) fn resolve(document: &Json, value: &Json) -> Json {
    let mut current = value.clone();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let Some(map) = current.as_object() else {
            return current;
        };
        let Some(reference) = non_empty(map.get("$ref")) else {
            return current;
        };
        if !reference.starts_with("#/") || seen.contains(&reference) {
            return current;
        }
        let target = reference[2..]
            .split('/')
            .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
            .try_fold(document, |item, segment| {
                item.as_object().and_then(|map| map.get(&segment))
            });
        let Some(target) = target else { return current };
        seen.push(reference);
        current = target.clone();
    }
}

pub(crate) fn project(document: &Json, value: &Json) -> Json {
    if !value.is_object() {
        return json!({});
    }
    let openapi_3_0 =
        non_empty(document.get("openapi")).is_some_and(|version| version.starts_with("3.0"));
    let (schema, definitions) = if openapi_3_0 {
        from_openapi_3_0(value)
    } else {
        from_openapi_3_1(value)
    };
    if definitions.is_empty() {
        return schema;
    }
    let mut schema = schema.as_object().cloned().unwrap_or_default();
    schema.insert("$defs".into(), Json::Object(definitions));
    Json::Object(schema)
}

pub(crate) fn component_definitions(document: &Json) -> Map<String, Json> {
    let schemas = document
        .get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(Json::as_object);
    schemas
        .map(|schemas| {
            entries(schemas)
                .into_iter()
                .map(|(name, value)| (name.clone(), project(document, value)))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn with_definitions(schema: Json, definitions: &Map<String, Json>) -> Json {
    if definitions.is_empty() {
        return schema;
    }
    let mut map = schema.as_object().cloned().unwrap_or_default();
    let mut merged = definitions.clone();
    if let Some(Json::Object(local)) = map.get("$defs") {
        for (key, value) in local {
            merged.insert(key.clone(), value.clone());
        }
    }
    map.insert("$defs".into(), Json::Object(merged));
    Json::Object(map)
}

pub(crate) fn media(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

pub(crate) fn is_json_media(media_type: &str) -> bool {
    let normalized = media(media_type);
    normalized == "application/json" || normalized.ends_with("+json")
}

pub(crate) fn is_binary_media(document: &Json, media_type: &str, value: &Json) -> bool {
    let normalized = media(media_type);
    if !is_json_media(&normalized) && !normalized.starts_with("text/") {
        return true;
    }
    let Some(map) = value.as_object() else {
        return false;
    };
    let schema = resolve(document, map.get("schema").unwrap_or(&Json::Null));
    schema.get("format").and_then(Json::as_str) == Some("binary")
}

fn operation_parameters(
    document: &Json,
    path_item: &Map<String, Json>,
    operation: &Map<String, Json>,
) -> Parsed<Vec<InputField>> {
    let mut declared: Vec<(String, String, Map<String, Json>)> = Vec::new();
    for raw in array(path_item.get("parameters"))
        .iter()
        .chain(array(operation.get("parameters")).iter())
    {
        let resolved = resolve(document, raw);
        let Some(parameter) = resolved.as_object() else {
            return Err("parameter declaration is invalid or unresolved".into());
        };
        let (Some(name), Some(location)) = (
            non_empty(parameter.get("name")),
            non_empty(parameter.get("in")),
        ) else {
            return Err("parameter declaration is missing name or location".into());
        };
        match declared
            .iter_mut()
            .find(|(existing, at, _)| *existing == name && *at == location)
        {
            Some(entry) => entry.2 = parameter.clone(),
            None => declared.push((name, location, parameter.clone())),
        }
    }
    let mut unordered = Vec::new();
    for (name, location, parameter) in declared {
        if location == "cookie" {
            return Err(format!("cookie parameter '{name}' is not supported"));
        }
        let location = match location.as_str() {
            "path" => Location::Path,
            "query" => Location::Query,
            "header" => Location::Header,
            other => {
                return Err(format!(
                    "parameter '{name}' uses unsupported location '{other}'"
                ));
            }
        };
        if location == Location::Header && IGNORED_HEADERS.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        if !parameter.contains_key("schema") && !parameter.contains_key("content") {
            return Err(format!(
                "parameter '{name}' declares neither schema nor content"
            ));
        }
        if parameter.contains_key("content") {
            return Err(format!(
                "parameter '{name}' uses unsupported content encoding"
            ));
        }
        if parameter.contains_key("style") && non_empty(parameter.get("style")).is_none() {
            return Err(format!("parameter '{name}' has an invalid style"));
        }
        if parameter
            .get("explode")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(format!("parameter '{name}' has an invalid explode value"));
        }
        if parameter
            .get("allowReserved")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(format!(
                "parameter '{name}' has an invalid allowReserved value"
            ));
        }
        if parameter.get("allowReserved") == Some(&Json::Bool(true)) {
            return Err(format!(
                "parameter '{name}' uses unsupported allowReserved encoding"
            ));
        }
        let declared_style = non_empty(parameter.get("style")).unwrap_or_else(|| {
            if location == Location::Query {
                "form"
            } else {
                "simple"
            }
            .into()
        });
        if location == Location::Query && declared_style != "form" && declared_style != "deepObject"
        {
            return Err(format!(
                "query parameter '{name}' uses unsupported style '{declared_style}'"
            ));
        }
        if location != Location::Query && declared_style != "simple" {
            return Err(format!(
                "{} parameter '{name}' uses unsupported style '{declared_style}'",
                location.as_str()
            ));
        }
        let style = match declared_style.as_str() {
            "deepObject" => Style::DeepObject,
            "form" => Style::Form,
            _ => Style::Simple,
        };
        let explode = parameter
            .get("explode")
            .and_then(Json::as_bool)
            .unwrap_or(style == Style::Form);
        if style == Style::DeepObject && !explode {
            return Err(format!(
                "query parameter '{name}' uses deepObject with explode=false"
            ));
        }
        let mut schema = project(document, parameter.get("schema").unwrap_or(&Json::Null));
        if let (Some(description), Some(map)) = (
            non_empty(parameter.get("description")),
            schema.as_object_mut(),
        ) && !map.contains_key("description")
        {
            map.insert("description".into(), Json::String(description));
        }
        unordered.push(InputField {
            input_name: String::new(),
            name,
            location,
            required: parameter.get("required") == Some(&Json::Bool(true))
                || location == Location::Path,
            schema,
            style: Some(style),
            explode: Some(explode),
        });
    }
    Ok([Location::Path, Location::Query, Location::Header]
        .into_iter()
        .flat_map(|location| {
            unordered
                .iter()
                .filter(move |field| field.location == location)
                .cloned()
        })
        .collect())
}

fn flattenable(schema: &Json, required: bool) -> Option<&Map<String, Json>> {
    let map = schema.as_object()?;
    let properties = map.get("properties")?.as_object()?;
    let ok = required
        && map.get("type") == Some(&Json::String("object".into()))
        && map.get("additionalProperties") == Some(&Json::Bool(false))
        && map.get("nullable") != Some(&Json::Bool(true))
        && !map.contains_key("allOf")
        && !map.contains_key("anyOf")
        && !map.contains_key("oneOf");
    ok.then_some(properties)
}

fn operation_body(
    document: &Json,
    operation: &Map<String, Json>,
) -> Parsed<(Vec<InputField>, Option<Body>)> {
    let resolved = resolve(
        document,
        operation.get("requestBody").unwrap_or(&Json::Null),
    );
    let Some(request) = resolved.as_object() else {
        return Ok((Vec::new(), None));
    };
    let content = request
        .get("content")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let selected = entries(&content)
        .into_iter()
        .find(|(media_type, _)| is_json_media(media_type));
    let Some((media_type, entry)) = selected.filter(|(_, entry)| entry.is_object()) else {
        let declared = entries(&content)
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "request body has no JSON content (declared: {})",
            if declared.is_empty() {
                "none".into()
            } else {
                declared
            }
        ));
    };
    let raw_schema = entry.get("schema").cloned().unwrap_or(Json::Null);
    let schema = resolve(document, &raw_schema);
    let required = request.get("required") == Some(&Json::Bool(true));
    let Some(properties) = flattenable(&schema, required) else {
        let field = InputField {
            input_name: String::new(),
            name: "body".into(),
            location: Location::Body,
            required,
            schema: project(document, &raw_schema),
            style: None,
            explode: None,
        };
        return Ok((
            vec![field],
            Some(Body {
                required,
                mode: BodyMode::Value,
                media_type: media_type.clone(),
            }),
        ));
    };
    let required_properties: Vec<&str> = schema
        .get("required")
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    let fields = entries(properties)
        .into_iter()
        .map(|(name, value)| InputField {
            input_name: String::new(),
            name: name.clone(),
            location: Location::Body,
            required: required && required_properties.contains(&name.as_str()),
            schema: project(document, value),
            style: None,
            explode: None,
        })
        .collect();
    Ok((
        fields,
        Some(Body {
            required,
            mode: BodyMode::Object,
            media_type: media_type.clone(),
        }),
    ))
}

pub(crate) fn operation_input(
    document: &Json,
    path_item: &Map<String, Json>,
    operation: &Map<String, Json>,
) -> Parsed<(Vec<InputField>, Option<Body>)> {
    let mut fields = operation_parameters(document, path_item, operation)?;
    let (body_fields, body) = operation_body(document, operation)?;
    fields.extend(body_fields);
    let conflicts: Vec<String> = fields
        .iter()
        .filter(|field| {
            fields
                .iter()
                .any(|other| other.name == field.name && other.location != field.location)
        })
        .map(|field| field.name.clone())
        .collect();
    let mut used: Vec<String> = Vec::new();
    for field in &mut fields {
        let visible = if is_blocked_member(&field.name) {
            format!("{}_2", field.name)
        } else {
            field.name.clone()
        };
        let base = if conflicts.contains(&field.name) {
            format!("{}_{visible}", field.location.as_str())
        } else {
            visible
        };
        let mut index = 1;
        let input_name = loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{base}_{index}")
            };
            if !used.contains(&candidate) {
                break candidate;
            }
            index += 1;
        };
        used.push(input_name.clone());
        field.input_name = input_name;
    }
    Ok((fields, body))
}

pub(crate) fn input_schema(fields: &[InputField], definitions: &Map<String, Json>) -> Json {
    let required: Vec<Json> = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| Json::String(field.input_name.clone()))
        .collect();
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert(
        "properties".into(),
        Json::Object(
            fields
                .iter()
                .map(|field| (field.input_name.clone(), field.schema.clone()))
                .collect(),
        ),
    );
    if !required.is_empty() {
        schema.insert("required".into(), Json::Array(required));
    }
    with_definitions(Json::Object(schema), definitions)
}
