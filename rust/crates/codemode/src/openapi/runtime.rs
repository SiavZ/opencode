//! Purpose: OpenAPI operation invocation over an explicit host `HttpClient` capability: request
//! construction (path/query/header/body serialization), host auth resolution and carriers,
//! response size limits, decoding and safe error summaries.
//! Scope: codemode/src/openapi/runtime.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/tools/mod.rs
//! `-- codemode/src/tools/render.rs
//! External:
//! |-- base64::Engine
//! |-- percent_encoding
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- codemode/src/openapi/mod.rs
//! Invariants: every model-controlled value is validated before auth resolution; credentials
//! never appear in tool input or failure messages; responses above 50 MiB are rejected.
//! Contract tests: codemode/tests/openapi.rs

use super::{
    AppliedAuth, BodyMode, Credential, HttpRequest, Location, Plan, SecurityScheme, Style,
};
use crate::tools::render::{entries, json_stringify};
use crate::tools::{ToolError, tool_error};
use base64::Engine;
use serde_json::{Map, Value as Json};

const MAX_ERROR_BODY_CHARS: usize = 1_024;
pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 50 * 1024 * 1024;

fn own<'a>(input: &'a Map<String, Json>, key: &str) -> Option<&'a Json> {
    input.get(key)
}

/// `String(value)` for JSON scalars.
fn scalar_string(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => json_stringify(other),
    }
}

const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// `encodeURIComponent` with `!'()*` also percent-encoded (RFC 3986 unreserved only).
fn encode_path(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, COMPONENT).to_string()
}

fn serialize_simple(
    field: &super::InputField,
    value: &Json,
    encode: fn(&str) -> String,
) -> Result<String, ToolError> {
    let nested = || {
        tool_error(format!(
            "Parameter '{}' contains an unsupported nested value.",
            field.input_name
        ))
    };
    let scalar = |item: &Json| match item {
        Json::Array(_) | Json::Object(_) => Err(nested()),
        other => Ok(encode(&scalar_string(other))),
    };
    match value {
        Json::Array(items) => Ok(items
            .iter()
            .map(scalar)
            .collect::<Result<Vec<_>, _>>()?
            .join(",")),
        Json::Object(map) => {
            let mut parts = Vec::new();
            for (name, item) in entries(map) {
                let rendered = scalar(item)?;
                if field.explode == Some(true) {
                    parts.push(format!("{}={rendered}", encode(name)));
                } else {
                    parts.push(encode(name));
                    parts.push(rendered);
                }
            }
            Ok(parts.join(","))
        }
        other => scalar(other),
    }
}

fn identity(value: &str) -> String {
    value.to_string()
}

fn serialize_query(
    field: &super::InputField,
    value: &Json,
    query: &mut Vec<(String, String)>,
) -> Result<(), ToolError> {
    let nested = |kind: &str| {
        tool_error(format!(
            "{kind} parameter '{}' contains an unsupported nested value.",
            field.input_name
        ))
    };
    if field.style == Some(Style::DeepObject) {
        let Some(map) = value.as_object() else {
            return Err(tool_error(format!(
                "Deep-object parameter '{}' must be an object.",
                field.input_name
            )));
        };
        for (name, item) in entries(map) {
            if item.is_object() || item.is_array() {
                return Err(nested("Deep-object"));
            }
            query.push((format!("{}[{name}]", field.name), scalar_string(item)));
        }
        return Ok(());
    }
    match value {
        Json::Array(items) => {
            let rendered = serialize_simple(field, value, identity)?;
            if field.explode != Some(true) {
                query.push((field.name.clone(), rendered));
                return Ok(());
            }
            for item in items {
                query.push((field.name.clone(), scalar_string(item)));
            }
            Ok(())
        }
        Json::Object(map) if field.explode == Some(true) => {
            for (name, item) in entries(map) {
                if item.is_object() || item.is_array() {
                    return Err(nested("Query"));
                }
                query.push((name.clone(), scalar_string(item)));
            }
            Ok(())
        }
        other => {
            query.push((
                field.name.clone(),
                serialize_simple(field, other, identity)?,
            ));
            Ok(())
        }
    }
}

fn build_url(plan: &Plan, input: &Map<String, Json>) -> Result<String, ToolError> {
    let mut url = plan.url.clone();
    for field in plan
        .fields
        .iter()
        .filter(|field| field.location == Location::Path)
    {
        let Some(item) = own(input, &field.input_name) else {
            return Err(tool_error(format!(
                "Missing required path parameter '{}'.",
                field.input_name
            )));
        };
        let value = serialize_simple(field, item, encode_path)?;
        // '.'/'..' survive encoding and would let a model retarget the request after URL
        // normalization collapses them.
        if value.is_empty() || value == "." || value == ".." {
            return Err(tool_error(format!(
                "Invalid path parameter '{}'.",
                field.input_name
            )));
        }
        url = url.replace(&format!("{{{}}}", field.name), &value);
    }
    if let Some(open) = url.find('{')
        && let Some(close) = url[open..].find('}').filter(|close| *close > 1)
    {
        return Err(tool_error(format!(
            "Unresolved path parameter {}.",
            &url[open..=open + close]
        )));
    }
    Ok(url)
}

/// Validates every model-controlled value and builds the request (before auth resolution).
pub(crate) fn build_request(
    plan: &Plan,
    input: &Map<String, Json>,
) -> Result<HttpRequest, ToolError> {
    let url = build_url(plan, input)?;
    if let Some(missing) = plan.fields.iter().find(|field| {
        field.required
            && field.location != Location::Path
            && own(input, &field.input_name).is_none()
    }) {
        let label = if missing.location == Location::Body {
            "body field".to_string()
        } else {
            format!("{} parameter", missing.location.as_str())
        };
        return Err(tool_error(format!(
            "Missing required {label} '{}'.",
            missing.input_name
        )));
    }
    let mut query = Vec::new();
    for field in plan
        .fields
        .iter()
        .filter(|field| field.location == Location::Query)
    {
        if let Some(item) = own(input, &field.input_name) {
            serialize_query(field, item, &mut query)?;
        }
    }
    let mut headers: Vec<(String, String)> = plan
        .headers
        .iter()
        .map(|(name, value)| (name.to_lowercase(), value.clone()))
        .collect();
    let mut set_header = |name: &str, value: String| {
        let name = name.to_lowercase();
        headers.retain(|(existing, _)| *existing != name);
        headers.push((name, value));
    };
    for field in plan
        .fields
        .iter()
        .filter(|field| field.location == Location::Header)
    {
        if let Some(item) = own(input, &field.input_name) {
            set_header(&field.name, serialize_simple(field, item, identity)?);
        }
    }
    let body = match &plan.body {
        Some(body) if body.mode == BodyMode::Value => plan
            .fields
            .iter()
            .find(|field| field.location == Location::Body)
            .and_then(|field| own(input, &field.input_name))
            .cloned(),
        Some(body) => {
            let fields: Map<String, Json> = plan
                .fields
                .iter()
                .filter(|field| field.location == Location::Body)
                .filter_map(|field| {
                    own(input, &field.input_name).map(|item| (field.name.clone(), item.clone()))
                })
                .collect();
            (body.required || !fields.is_empty()).then_some(Json::Object(fields))
        }
        None => None,
    };
    let body = match (body, &plan.body) {
        (Some(value), Some(spec)) => {
            set_header("content-type", spec.media_type.clone());
            Some(json_stringify(&value).into_bytes())
        }
        _ => None,
    };
    Ok(HttpRequest {
        method: plan.operation.method.clone(),
        url,
        query,
        headers,
        body,
    })
}

pub(crate) async fn resolve_auth(plan: &Plan) -> Result<AppliedAuth, ToolError> {
    if plan.security.is_empty() {
        return Ok(AppliedAuth::default());
    }
    let mut unavailable: Vec<String> = Vec::new();
    'alternatives: for requirement in &plan.security {
        if requirement.is_empty() {
            return Ok(AppliedAuth::default());
        }
        let mut credentials = Vec::new();
        for (name, scopes) in requirement {
            let scheme = plan
                .schemes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, scheme)| scheme.clone());
            let (Some(scheme), Some(auth)) = (scheme, plan.auth.as_ref()) else {
                unavailable.push(name.clone());
                continue 'alternatives;
            };
            let context = super::AuthContext {
                name: name.clone(),
                definition: scheme.clone(),
                scopes: scopes.clone(),
                operation: plan.operation.clone(),
            };
            match auth.resolve(context).await? {
                Some(credential) => credentials.push((name.clone(), scheme, credential)),
                None => {
                    unavailable.push(name.clone());
                    continue 'alternatives;
                }
            }
        }
        return apply_credentials(credentials);
    }
    let mut names: Vec<String> = Vec::new();
    for name in unavailable {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Err(tool_error(format!(
        "{} {} requires authentication; no credential available for: {}.",
        plan.operation.method,
        plan.operation.path,
        names.join(", ")
    )))
}

fn apply_credentials(
    credentials: Vec<(String, SecurityScheme, Credential)>,
) -> Result<AppliedAuth, ToolError> {
    let mut applied = AppliedAuth::default();
    let add = |target: &mut Vec<(String, String)>, carrier: &str, name: String, value: String| {
        if target.iter().any(|(existing, _)| *existing == name) {
            return Err(tool_error(format!(
                "Authentication resolves multiple credentials for {carrier} '{name}'."
            )));
        }
        target.push((name, value));
        Ok(())
    };
    for (name, definition, credential) in credentials {
        match credential {
            Credential::Bearer { token } => add(
                &mut applied.headers,
                "header",
                "authorization".into(),
                format!("Bearer {token}"),
            )?,
            Credential::Basic { username, password } => {
                let encoded = base64::engine::general_purpose::STANDARD
                    .encode(format!("{username}:{password}"));
                add(
                    &mut applied.headers,
                    "header",
                    "authorization".into(),
                    format!("Basic {encoded}"),
                )?;
            }
            Credential::Header {
                name: header,
                value,
            } => add(&mut applied.headers, "header", header.to_lowercase(), value)?,
            Credential::ApiKey { value } => {
                let SecurityScheme::ApiKey {
                    name: parameter,
                    location,
                } = definition
                else {
                    return Err(tool_error(format!(
                        "Security scheme '{name}' is not an apiKey scheme; resolve a bearer, basic, or header credential for it."
                    )));
                };
                match location.as_str() {
                    "cookie" => {
                        return Err(tool_error(format!(
                            "Cookie authentication '{name}' is not supported."
                        )));
                    }
                    "header" => add(
                        &mut applied.headers,
                        "header",
                        parameter.to_lowercase(),
                        value,
                    )?,
                    _ => add(&mut applied.query, "query", parameter, value)?,
                }
            }
        }
    }
    Ok(applied)
}

/// Decodes a response: size limit, JSON/text by media type, and safe HTTP error summaries.
pub(crate) fn read_response(plan: &Plan, response: super::HttpResponse) -> Result<Json, ToolError> {
    let operation = format!("{} {}", plan.operation.method, plan.operation.path);
    let header = |name: &str| {
        response
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    let declared = header("content-length").and_then(|value| value.trim().parse::<u64>().ok());
    if declared.is_some_and(|size| size > MAX_RESPONSE_BODY_BYTES as u64)
        || response.body.len() > MAX_RESPONSE_BODY_BYTES
    {
        return Err(tool_error(format!("{operation} response exceeds 50 MiB.")));
    }
    let text = String::from_utf8_lossy(&response.body).into_owned();
    let media = header("content-type")
        .map(|value| value.split(';').next().unwrap_or("").trim().to_lowercase());
    let json = media
        .as_deref()
        .is_some_and(|media| media == "application/json" || media.ends_with("+json"));
    let decoded: Option<Json> = if text.is_empty() {
        Some(Json::Null)
    } else if json {
        serde_json::from_str(&text).ok()
    } else {
        None
    };
    let parsed = if json {
        decoded
            .clone()
            .unwrap_or_else(|| Json::String(text.clone()))
    } else if text.is_empty() {
        Json::Null
    } else {
        Json::String(text.clone())
    };
    if !(200..300).contains(&response.status) {
        let rendered = match &parsed {
            Json::String(text) => text.clone(),
            other => json_stringify(other),
        };
        let summary = if rendered.is_empty() || rendered == "null" {
            "no response body".to_string()
        } else if rendered.encode_utf16().count() > MAX_ERROR_BODY_CHARS {
            format!(
                "{}...",
                crate::value::text::slice16(&rendered, 0, MAX_ERROR_BODY_CHARS)
            )
        } else {
            rendered
        };
        return Err(tool_error(format!(
            "{operation} failed with HTTP {}: {summary}",
            response.status
        )));
    }
    if json && decoded.is_none() {
        return Err(tool_error(format!("{operation} returned malformed JSON.")));
    }
    Ok(parsed)
}
