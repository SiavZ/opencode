//! Purpose: OpenAI Responses streaming event schema (packages/llm/src/protocols/openai-responses.ts
//! `OpenAIResponsesEvent`, `OpenAIResponsesStreamItem`, `OpenAIResponsesUsage`) and the pure
//! mappers over it: usage, finish reason, hosted (provider-executed) tool events and
//! provider-error messages with context-overflow classification.
//! Scope: decoding types plus stateless mapping functions.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/transport/overflow.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! `-- provider-adapters/src/protocols/openai/responses_stream.rs
//! Invariants: unknown event types and unknown item fields are accepted and ignored. Hosted
//! tool results round-trip the decoded item (schema-known keys only, in schema order) as
//! `json`, or `error` when `item.error` is non-null. Provider-error messages prefer
//! `code: message`, then message, then code, then the fallback.
//! Contract tests: provider-adapters/tests/openai_responses.rs

use opencode_llm_api::{
    FinishReason, LlmEvent, ProviderFailureClassification, ToolResultValue, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::protocols::openai::wire::{flat, nullable, openai_metadata, present, ser_js_num};
use crate::shared::{subtract_tokens, total_tokens};
use crate::transport::overflow::is_context_overflow;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CachedDetails {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    pub cached_tokens: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ReasoningDetails {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_tokens: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ResponsesUsage {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_tokens_details: Option<Option<CachedDetails>>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_tokens_details: Option<Option<ReasoningDetails>>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_tokens: Option<f64>,
}

/// `OpenAIResponsesStreamItem`. Serializes back to the schema-known subset.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StreamItem {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub call_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub arguments: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub action: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub queries: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub results: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub code: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub container_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub outputs: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub server_label: Option<String>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub output: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<Value>,
    #[serde(
        default,
        deserialize_with = "nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub encrypted_content: Option<Option<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ErrorPayload {
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub code: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub message: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub param: Option<Option<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IncompleteDetails {
    pub reason: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ResponseInfo {
    #[serde(default, deserialize_with = "present::deserialize")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub service_tier: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub incomplete_details: Option<Option<IncompleteDetails>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub usage: Option<Option<ResponsesUsage>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    pub error: Option<Option<ErrorPayload>>,
}

/// `OpenAIResponsesEvent`.
#[derive(Clone, Debug, Deserialize)]
pub struct ResponsesEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub delta: Option<String>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub item_id: Option<String>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub summary_index: Option<f64>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub item: Option<StreamItem>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub response: Option<ResponseInfo>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub code: Option<String>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub message: Option<String>,
    #[serde(default, deserialize_with = "present::deserialize")]
    pub param: Option<String>,
}

/// `TERMINAL_TYPES`: clean finishes and hard failures that end the stream.
pub const TERMINAL_TYPES: &[&str] = &[
    "response.completed",
    "response.incomplete",
    "response.failed",
];

/// `mapUsage`.
pub fn map_usage(usage: Option<&ResponsesUsage>) -> Option<Usage> {
    let usage = usage?;
    let cached = flat(&usage.input_tokens_details).and_then(|d| d.cached_tokens);
    let reasoning = flat(&usage.output_tokens_details).and_then(|d| d.reasoning_tokens);
    let metadata = match serde_json::to_value(usage) {
        Ok(Value::Object(map)) => Some(openai_metadata(map)),
        _ => None,
    };
    Some(Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        non_cached_input_tokens: subtract_tokens(usage.input_tokens, cached),
        cache_read_input_tokens: cached,
        cache_write_input_tokens: None,
        reasoning_tokens: reasoning,
        total_tokens: total_tokens(usage.input_tokens, usage.output_tokens, usage.total_tokens),
        provider_metadata: metadata,
    })
}

/// `mapFinishReason`.
pub fn map_finish_reason(event: &ResponsesEvent, has_function_call: bool) -> FinishReason {
    let reason = event
        .response
        .as_ref()
        .and_then(|r| flat(&r.incomplete_details))
        .map(|d| d.reason.as_str());
    match reason {
        None => {
            if has_function_call {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            }
        }
        Some("max_output_tokens") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(_) if has_function_call => FinishReason::ToolCalls,
        Some(_) => FinishReason::Unknown,
    }
}

/// `HOSTED_TOOLS`: common name and typed input for provider-executed tool items.
pub fn hosted_tool(item: &StreamItem) -> Option<(&'static str, Value)> {
    let action = || item.action.clone().unwrap_or_else(|| json!({}));
    let fields = |pairs: &[(&str, Option<Value>)]| {
        let mut map = Map::new();
        for (k, v) in pairs {
            if let Some(v) = v {
                map.insert((*k).to_owned(), v.clone());
            }
        }
        Value::Object(map)
    };
    let tool = match item.kind.as_str() {
        "web_search_call" => ("web_search", action()),
        "web_search_preview_call" => ("web_search_preview", action()),
        "file_search_call" => (
            "file_search",
            json!({ "queries": item.queries.clone().unwrap_or_else(|| json!([])) }),
        ),
        "code_interpreter_call" => (
            "code_interpreter",
            fields(&[
                ("code", item.code.clone().map(Value::from)),
                ("container_id", item.container_id.clone().map(Value::from)),
            ]),
        ),
        "computer_use_call" => ("computer_use", action()),
        "image_generation_call" => ("image_generation", json!({})),
        "mcp_call" => (
            "mcp",
            fields(&[
                ("server_label", item.server_label.clone().map(Value::from)),
                ("name", item.name.clone().map(Value::from)),
                ("arguments", item.arguments.clone().map(Value::from)),
            ]),
        ),
        "local_shell_call" => ("local_shell", action()),
        _ => return None,
    };
    item.id.as_deref().filter(|id| !id.is_empty())?;
    Some(tool)
}

/// `hostedToolEvents`: a `tool-call` + `tool-result` pair marked `providerExecuted`.
pub fn hosted_tool_events(item: &StreamItem, name: &str, input: Value) -> Vec<LlmEvent> {
    let id = item.id.clone().unwrap_or_default();
    let metadata = Some(openai_metadata(Map::from_iter([(
        "itemId".to_owned(),
        Value::from(id.clone()),
    )])));
    let result = match item.error.as_ref().filter(|e| !e.is_null()) {
        Some(error) => ToolResultValue::Error(error.clone()),
        None => ToolResultValue::Json(serde_json::to_value(item).unwrap_or(Value::Null)),
    };
    vec![
        LlmEvent::ToolCall {
            id: id.clone(),
            name: name.to_owned(),
            input,
            provider_executed: Some(true),
            provider_metadata: metadata.clone(),
        },
        LlmEvent::ToolResult {
            id,
            name: name.to_owned(),
            result,
            output: None,
            provider_executed: Some(true),
            provider_metadata: metadata,
        },
    ]
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|s| !s.is_empty())
}

/// `providerError`: `code: message` when both are known, context overflow classified.
pub fn provider_error(event: &ResponsesEvent, fallback: &str) -> LlmEvent {
    let nested = event.response.as_ref().and_then(|r| flat(&r.error));
    let message = non_empty(event.message.as_ref())
        .or_else(|| nested.and_then(|n| non_empty(flat(&n.message))));
    let code =
        non_empty(event.code.as_ref()).or_else(|| nested.and_then(|n| non_empty(flat(&n.code))));
    let text = match (message, code) {
        (Some(m), Some(c)) => format!("{c}: {m}"),
        (Some(m), None) => m.to_owned(),
        (None, Some(c)) => c.to_owned(),
        (None, None) => fallback.to_owned(),
    };
    let overflow = code == Some("context_length_exceeded") || is_context_overflow(&text);
    LlmEvent::ProviderError {
        message: text,
        classification: overflow.then_some(ProviderFailureClassification::ContextOverflow),
        retryable: None,
        provider_metadata: None,
    }
}
