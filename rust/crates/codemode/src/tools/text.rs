//! Purpose: shared text helpers for model-visible rendering: JS identifiers, `JSON.stringify`
//! with JS number formatting, JS own-key ordering, and JSDoc comment blocks.
//! Scope: `tool-schema.ts` key/JSON/JSDoc formatting helpers.
//! Uses:
//! rust/crates/
//! |-- codemode/src/value/number.rs
//! |-- codemode/src/value/object.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- f64::NAN
//! |-- number::format
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- codemode/src/tools/mod.rs
//! `-- codemode/src/tools/render.rs
//! Invariants: JSDoc output never contains a raw `*/` terminator from user text.
//! Contract tests: codemode/tests/signature.rs

use crate::value::number;
use crate::value::object::js_key_order;
use serde_json::{Map, Value as Json};

/// Bare identifier usable unquoted as an object key or dotted tool-path segment.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

/// `JSON.stringify` for JSON values with JS number formatting.
pub(crate) fn json_stringify(value: &Json) -> String {
    match value {
        Json::Null => "null".into(),
        Json::Bool(flag) => flag.to_string(),
        Json::Number(num) => match (num.as_i64(), num.as_u64()) {
            (Some(int), _) => int.to_string(),
            (_, Some(int)) => int.to_string(),
            _ => number::format(num.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(text) => crate::value::text::quote_json(text),
        Json::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(json_stringify)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Json::Object(map) => format!(
            "{{{}}}",
            entries(map)
                .into_iter()
                .map(|(key, item)| format!(
                    "{}:{}",
                    crate::value::text::quote_json(key),
                    json_stringify(item)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Object entries in JS own-key order (integer-like keys first).
pub(crate) fn entries(map: &Map<String, Json>) -> Vec<(&String, &Json)> {
    js_key_order(map.keys().cloned())
        .into_iter()
        .filter_map(|key| map.get_key_value(&key))
        .collect()
}

pub(crate) fn doc_tags(schema: &Json) -> Vec<String> {
    let empty = Map::new();
    let view = schema.as_object().unwrap_or(&empty);
    let mut tags = Vec::new();
    if view.get("deprecated") == Some(&Json::Bool(true)) {
        tags.push("@deprecated".to_string());
    }
    if let Some(default) = view.get("default") {
        tags.push(format!("@default {}", json_stringify(default)));
    }
    if let Some(Json::String(format)) = view.get("format") {
        tags.push(format!("@format {format}"));
    }
    for key in ["minItems", "maxItems"] {
        if let Some(Json::Number(value)) = view.get(key) {
            tags.push(format!(
                "@{key} {}",
                number::format(value.as_f64().unwrap_or(f64::NAN))
            ));
        }
    }
    tags
}

pub(crate) fn jsdoc(description: Option<&str>, tags: &[String], pad: &str) -> String {
    let mut lines: Vec<String> = description
        .map(|text| text.split('\n').map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(tags.iter().cloned())
        .map(|line| {
            line.replace("*/", "* /")
                .trim_end_matches(crate::value::text::is_js_whitespace)
                .to_string()
        })
        .collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    match lines.len() {
        0 => String::new(),
        1 => format!("{pad}/** {} */\n", lines[0]),
        _ => {
            let body = lines
                .iter()
                .map(|line| {
                    if line.is_empty() {
                        format!("{pad} *")
                    } else {
                        format!("{pad} * {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("{pad}/**\n{body}\n{pad} */\n")
        }
    }
}
