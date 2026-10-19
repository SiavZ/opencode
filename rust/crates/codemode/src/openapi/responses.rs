//! Purpose: OpenAPI successful-response analysis: 2xx selection and ordering, transport
//! rejection (WebSocket, SSE, binary) and output schema projection.
//! Scope: codemode/src/openapi/spec.ts successfulResponses/operationOutput.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/spec.rs
//! |-- codemode/src/tools/render.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- codemode/src/openapi/mod.rs
//! Invariants: refs are resolved before transport detection; unrepresentable responses skip the
//! operation instead of producing a misleading output type.
//! Contract tests: codemode/tests/openapi.rs

use super::spec::{
    Parsed, is_binary_media, is_json_media, media, non_empty, project, resolve, with_definitions,
};
use crate::tools::render::entries;
use serde_json::{Map, Value as Json, json};

fn successful_responses(
    document: &Json,
    operation: &Map<String, Json>,
) -> Parsed<Vec<Map<String, Json>>> {
    let Some(responses) = operation.get("responses").and_then(Json::as_object) else {
        return Ok(Vec::new());
    };
    let ordered = entries(responses);
    let is_2xx = |status: &str| {
        status.len() == 3
            && status.starts_with('2')
            && status.bytes().all(|byte| byte.is_ascii_digit())
    };
    let mut selected: Vec<(&String, &Json)> = ordered
        .iter()
        .copied()
        .filter(|(status, _)| is_2xx(status))
        .collect();
    selected.sort_by(|left, right| crate::value::text::locale_compare(left.0, right.0));
    selected.extend(
        ordered
            .iter()
            .copied()
            .filter(|(status, _)| status.to_uppercase() == "2XX"),
    );
    selected
        .into_iter()
        .map(|(_, value)| {
            let resolved = resolve(document, value);
            match resolved.as_object() {
                Some(map) if non_empty(map.get("$ref")).is_none() => Ok(map.clone()),
                _ => Err("successful response declaration is invalid or unresolved".to_string()),
            }
        })
        .collect()
}

pub(crate) fn operation_output(
    document: &Json,
    operation: &Map<String, Json>,
    definitions: &Map<String, Json>,
) -> Parsed<Option<Json>> {
    if operation.get("x-websocket") == Some(&Json::Bool(true)) {
        return Err("WebSocket operations are not supported".into());
    }
    let responses = successful_responses(document, operation)?;
    let content_of =
        |response: &Map<String, Json>| response.get("content").and_then(Json::as_object).cloned();
    if responses.iter().filter_map(content_of).any(|content| {
        content
            .keys()
            .any(|media_type| media(media_type) == "text/event-stream")
    }) {
        return Err("SSE operations are not supported".into());
    }
    if responses.iter().filter_map(content_of).any(|content| {
        content
            .iter()
            .any(|(media_type, value)| is_binary_media(document, media_type, value))
    }) {
        return Err("binary responses are not supported".into());
    }
    let mut outcomes = Vec::new();
    for response in &responses {
        if response
            .get("content")
            .is_some_and(|content| !content.is_object())
        {
            return Ok(None);
        }
        let content = content_of(response).unwrap_or_default();
        if content.is_empty() {
            outcomes.push(json!({ "type": "null" }));
            continue;
        }
        for (media_type, value) in entries(&content) {
            if !is_json_media(media_type) {
                outcomes.push(json!({ "type": "string" }));
                continue;
            }
            let Some(schema) = value.as_object().and_then(|map| map.get("schema")) else {
                return Ok(None);
            };
            outcomes.push(project(document, schema));
        }
    }
    Ok(match outcomes.len() {
        0 => None,
        1 => Some(with_definitions(outcomes.remove(0), definitions)),
        _ => Some(with_definitions(json!({ "anyOf": outcomes }), definitions)),
    })
}
