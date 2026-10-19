//! Purpose: provider error classification from packages/opencode/src/provider/error.ts:
//! `parseStreamError` (Responses-style `{type:"error"}` stream payloads) and `parseAPICallError`
//! (HTTP failures: message extraction, HTML gateway pages, context overflow, OpenAI 404 retry).
//! Scope: pure functions over already-read status/body text. No I/O.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde_json::Value
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/tests/providers_transform.rs
//! Invariants: messages are byte-identical to the baseline, trimmed; overflow wins over
//! api_error when the message, status 413 or `error.code` says so.
//! Contract tests: provider-adapters/tests/providers_transform.rs

use indexmap::IndexMap;
use serde_json::Value;

use crate::transport::is_context_overflow;

#[derive(Clone, Debug, PartialEq)]
pub enum ParsedStreamError {
    ContextOverflow {
        message: String,
        response_body: String,
    },
    ApiError {
        message: String,
        is_retryable: bool,
        response_body: String,
    },
}

/// `json(input)`: parse a string to an object, or pass objects through.
fn json(input: &Value) -> Option<Value> {
    match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ (Value::Object(_) | Value::Array(_))) => Some(v),
            _ => None,
        },
        Value::Object(_) | Value::Array(_) => Some(input.clone()),
        _ => None,
    }
}

fn error_message(body: &Value, fallback: &str) -> String {
    body.pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

/// `parseStreamError(input)`.
pub fn parse_stream_error(input: &Value) -> Option<ParsedStreamError> {
    let raw = json(input)?;
    let body = match raw.get("message") {
        Some(Value::String(m)) => json(&Value::String(m.clone())).unwrap_or(raw),
        _ => raw,
    };
    let response_body = body.to_string();
    if body.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let api = |message: String, is_retryable: bool| ParsedStreamError::ApiError {
        message,
        is_retryable,
        response_body: response_body.clone(),
    };
    Some(match body.pointer("/error/code").and_then(Value::as_str) {
        Some("context_length_exceeded") => ParsedStreamError::ContextOverflow {
            message: "Input exceeds context window of this model".into(),
            response_body: response_body.clone(),
        },
        Some("insufficient_quota") => api(
            "Quota exceeded. Check your plan and billing details.".into(),
            false,
        ),
        Some("usage_not_included") => api(
            "To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus."
                .into(),
            false,
        ),
        Some("invalid_prompt") => api(error_message(&body, "Invalid prompt."), false),
        Some("server_is_overloaded" | "server_error") => {
            api(error_message(&body, "Server error."), true)
        }
        _ => api(error_message(&body, "Server error."), true),
    })
}

/// The `APICallError` fields the parser reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiCallError {
    pub message: String,
    pub status_code: Option<u16>,
    pub response_body: Option<String>,
    pub response_headers: Option<IndexMap<String, String>>,
    pub url: Option<String>,
    pub is_retryable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParsedApiCallError {
    ContextOverflow {
        message: String,
        response_body: Option<String>,
    },
    ApiError {
        message: String,
        status_code: Option<u16>,
        is_retryable: bool,
        response_headers: Option<IndexMap<String, String>>,
        response_body: Option<String>,
        metadata: Option<IndexMap<String, String>>,
    },
}

/// Node `http.STATUS_CODES` for the statuses providers return.
pub fn status_text(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => return None,
    })
}

fn is_html(body: &str) -> bool {
    let lower = body.trim_start().to_ascii_lowercase();
    lower.starts_with("<!doctype") || lower.starts_with("<html")
}

/// `message(providerID, e)`.
pub fn api_error_message(e: &ApiCallError) -> String {
    let msg = &e.message;
    let text = if msg.is_empty() {
        match (&e.response_body, e.status_code.and_then(status_text)) {
            (Some(body), _) if !body.is_empty() => body.clone(),
            (_, Some(text)) => text.to_owned(),
            _ => "Unknown error".into(),
        }
    } else {
        match e.response_body.as_deref().filter(|b| !b.is_empty()) {
            None => msg.clone(),
            Some(_)
                if e.status_code
                    .is_some_and(|s| Some(msg.as_str()) != status_text(s)) =>
            {
                msg.clone()
            }
            Some(body) => body_message(msg, body, e.status_code),
        }
    };
    text.trim().to_owned()
}

fn body_message(msg: &str, body: &str, status: Option<u16>) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(body) {
        let candidate = [
            parsed.get("message"),
            parsed.get("error"),
            parsed.pointer("/error/message"),
        ]
        .into_iter()
        .flatten()
        .find(|v| truthy(v));
        if let Some(Value::String(err)) = candidate {
            return format!("{msg}: {err}");
        }
    }
    if is_html(body) {
        return match status {
            Some(401) => "Unauthorized: request was blocked by a gateway or proxy. Your authentication token may be missing or expired — try running `opencode auth login <your provider URL>` to re-authenticate.".into(),
            Some(403) => "Forbidden: request was blocked by a gateway or proxy. You may not have permission to access this resource — check your account and provider settings.".into(),
            _ => msg.to_owned(),
        };
    }
    format!("{msg}: {body}")
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `parseAPICallError({ providerID, error })`.
pub fn parse_api_call_error(provider_id: &str, e: &ApiCallError) -> ParsedApiCallError {
    let message = api_error_message(e);
    let body = e
        .response_body
        .as_ref()
        .and_then(|b| json(&Value::String(b.clone())));
    let code_overflow = body
        .as_ref()
        .and_then(|b| b.pointer("/error/code"))
        .and_then(Value::as_str)
        == Some("context_length_exceeded");
    if is_context_overflow(&message) || e.status_code == Some(413) || code_overflow {
        return ParsedApiCallError::ContextOverflow {
            message,
            response_body: e.response_body.clone(),
        };
    }
    let is_retryable = if provider_id.starts_with("openai") {
        match e.status_code {
            Some(status) => status == 404 || e.is_retryable,
            None => e.is_retryable,
        }
    } else {
        e.is_retryable
    };
    ParsedApiCallError::ApiError {
        message,
        status_code: e.status_code,
        is_retryable,
        response_headers: e.response_headers.clone(),
        response_body: e.response_body.clone(),
        metadata: e
            .url
            .as_ref()
            .map(|u| IndexMap::from([("url".to_owned(), u.clone())])),
    }
}
