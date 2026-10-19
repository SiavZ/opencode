//! Purpose: OpenAPI operation tool-path naming (sanitization, synthesized ids, collision
//! handling) and server URL selection/validation.
//! Scope: codemode/src/openapi/spec.ts operationPath/serverUrl helpers.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/spec.rs
//! `-- codemode/src/value/convert.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- str::to_string
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! `-- codemode/src/openapi/mod.rs
//! Invariants: generated paths never collide with an existing tool or namespace and never use a
//! blocked member name; accepted base URLs are absolute http(s) without query or fragment.
//! Contract tests: codemode/tests/openapi.rs

use super::spec::{Parsed, array, non_empty};
use crate::value::convert::is_blocked_member;
use serde_json::{Map, Value as Json};

fn sanitize_segment(raw: &str) -> String {
    let mut replaced = String::new();
    let mut in_run = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
            replaced.push(ch);
            in_run = false;
        } else if !in_run {
            replaced.push('_');
            in_run = true;
        }
    }
    let trimmed = replaced.trim_matches('_');
    let base = match trimmed.chars().next() {
        None => "operation".to_string(),
        Some(first) if first.is_ascii_digit() => format!("_{trimmed}"),
        Some(_) => trimmed.to_string(),
    };
    if is_blocked_member(&base) {
        format!("{base}_2")
    } else {
        base
    }
}

fn fallback_operation_id(method: &str, path: &str) -> String {
    let mut words = vec![method.to_string()];
    for part in path.split('/').filter(|part| !part.is_empty()) {
        let parts: Vec<String> = match part
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        {
            Some(name) => vec!["by".into(), name.into()],
            None => vec![part.into()],
        };
        for item in parts {
            words.extend(
                item.split(|ch: char| !ch.is_ascii_alphanumeric())
                    .filter(|word| !word.is_empty())
                    .map(str::to_string),
            );
        }
    }
    words
        .into_iter()
        .enumerate()
        .map(|(index, word)| {
            let lower = word.to_lowercase();
            if index == 0 {
                return lower;
            }
            let mut chars = lower.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}

fn available(segments: &[String], used: &[String], namespaces: &[String]) -> bool {
    let key = segments.join(".");
    if used.contains(&key) || namespaces.contains(&key) {
        return false;
    }
    (1..segments.len()).all(|end| !used.contains(&segments[..end].join(".")))
}

pub(crate) fn operation_path(
    method: &str,
    path: &str,
    operation: &Map<String, Json>,
    used: &[String],
    namespaces: &[String],
) -> Vec<String> {
    let raw = non_empty(operation.get("operationId"));
    let segments: Vec<String> = match raw {
        None => vec![sanitize_segment(&fallback_operation_id(method, path))],
        Some(raw) => raw.split('.').map(sanitize_segment).collect(),
    };
    if available(&segments, used, namespaces) {
        return segments;
    }
    let conflict = (1..segments.len())
        .map(|end| end - 1)
        .find(|index| used.contains(&segments[..=*index].join(".")));
    if let Some(conflict) = conflict.filter(|conflict| conflict + 1 < segments.len()) {
        let next = &segments[conflict + 1];
        let mut capitalized = next.chars();
        let joined = format!(
            "{}{}",
            segments[conflict],
            capitalized.next().map_or_else(String::new, |first| first
                .to_uppercase()
                .chain(capitalized)
                .collect())
        );
        let collapsed: Vec<String> = segments
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != conflict + 1)
            .map(|(index, segment)| {
                if index == conflict {
                    joined.clone()
                } else {
                    segment.clone()
                }
            })
            .collect();
        if available(&collapsed, used, namespaces) {
            return collapsed;
        }
    }
    let fallback = segments.join("_");
    let mut index = 2;
    loop {
        let candidate = format!("{fallback}_{index}");
        if available(std::slice::from_ref(&candidate), used, namespaces) {
            return vec![candidate];
        }
        index += 1;
    }
}

pub(crate) fn spec_server_url(source: &Json) -> Parsed<String> {
    let server = array(source.get("servers"))
        .into_iter()
        .find(Json::is_object);
    let Some(url) = server.and_then(|server| non_empty(server.get("url"))) else {
        return Err("spec declares no servers; pass baseUrl".into());
    };
    if url
        .find('{')
        .is_some_and(|open| url[open..].find('}').is_some_and(|close| close > 1))
    {
        return Err(format!(
            "server URL '{url}' is not an absolute URL; pass baseUrl"
        ));
    }
    validate_base_url(&url)
}

pub(crate) fn validate_base_url(value: &str) -> Parsed<String> {
    let lower = value.to_lowercase();
    let not_absolute = || format!("server URL '{value}' is not an absolute HTTP(S) URL");
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(not_absolute());
    }
    let url = url::Url::parse(value).map_err(|_| not_absolute())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(not_absolute());
    }
    if url.query().is_some_and(|query| !query.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return Err(format!(
            "server URL '{value}' contains an unsupported query string or fragment"
        ));
    }
    Ok(value.to_string())
}
