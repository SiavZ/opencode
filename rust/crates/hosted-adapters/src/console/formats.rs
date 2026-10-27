//! Purpose: translate Zen provider requests, responses, SSE parts and billing usage.
//! Scope: native provider wire adapters matching console/app Zen TypeScript helpers.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats/bedrock.rs
//! |-- hosted-adapters/src/console/formats/provider.rs
//! |-- hosted-adapters/src/console/formats/request.rs
//! |-- hosted-adapters/src/console/formats/response.rs
//! `-- hosted-adapters/src/console/formats/stream.rs
//! External:
//! |-- http::header::InvalidHeaderValue
//! |-- serde::{Deserialize, Serialize}
//! |-- serde_json::{Value, json}
//! |-- thiserror::Error
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/mod.rs
//! `-- hosted-adapters/tests/console_formats.rs
//! Invariants: same-format payloads are opaque; unsupported cross-format conversions fail explicitly.
//! Contract tests: hosted-adapters/tests/console_formats.rs

mod bedrock;
mod provider;
mod request;
mod response;
mod stream;

pub use bedrock::BedrockDecoder;
pub use provider::{ProviderHelper, UsageInfo, UsageParser};
pub use stream::{StreamConverter, build_cost_chunk, convert_stream_part};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Wire format names used by Zen model/provider configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Format {
    #[serde(rename = "openai")]
    Openai,
    #[serde(rename = "oa-compat")]
    OaCompat,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "systemone")]
    Systemone,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("unsupported provider conversion from {from:?} to {to:?}")]
    Unsupported { from: Format, to: Format },
    #[error("invalid provider header: {0}")]
    Header(#[from] http::header::InvalidHeaderValue),
    #[error("invalid provider stream: {0}")]
    Stream(String),
    #[error("invalid provider request: {0}")]
    InvalidRequest(String),
}

pub fn convert_request(body: Value, from: Format, to: Format) -> Result<Value, FormatError> {
    if from == to {
        return Ok(body);
    }
    supported(from, to)?;
    if !body.is_object() {
        return Ok(body);
    }
    let common = request::from_request(body, from);
    if to == Format::Anthropic
        && array(&common["tools"])
            .iter()
            .any(|tool| tool["type"] == "function" && tool["function"].is_null())
    {
        return Err(FormatError::InvalidRequest(
            "Anthropic function tools require function metadata".into(),
        ));
    }
    Ok(request::to_request(common, to))
}

pub fn convert_response(body: Value, from: Format, to: Format) -> Result<Value, FormatError> {
    if from == to {
        return Ok(body);
    }
    supported(from, to)?;
    if !body.is_object() {
        return Ok(body);
    }
    let common = response::from_response(body, from);
    Ok(response::to_response(common, to))
}

fn supported(from: Format, to: Format) -> Result<(), FormatError> {
    if matches!(from, Format::Google | Format::Systemone)
        || matches!(to, Format::Google | Format::Systemone)
    {
        return Err(FormatError::Unsupported { from, to });
    }
    Ok(())
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
fn pick(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| {
                value
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect(),
    )
}
fn copy(out: &mut Value, key: &str, value: &Value, source: &str) {
    if let Some(value) = value.get(source) {
        out[key] = value.clone();
    }
}
fn stringify(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn parsed(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_else(|| value.clone())
}
fn generated(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn image(part: &Value) -> Option<Value> {
    if matches!(text(&part["type"]), "image_url" | "input_image") && truthy(&part["image_url"]) {
        return Some(json!({"type":"image_url","image_url":part["image_url"]}));
    }
    let source = &part["source"];
    match text(&source["type"]) {
        "url" if source["url"].is_string() => {
            Some(json!({"type":"image_url","image_url":{"url":source["url"]}}))
        }
        "base64" if source["media_type"].is_string() && source["data"].is_string() => Some(
            json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",text(&source["media_type"]),text(&source["data"]))}}),
        ),
        _ => None,
    }
}
