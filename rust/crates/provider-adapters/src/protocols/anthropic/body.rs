//! Purpose: lower an option-resolved `LlmRequest` into the Anthropic Messages request body
//! (packages/llm/src/protocols/anthropic-messages.ts `fromRequest` and helpers): system,
//! messages, media, tool use/result, server tool round-trip, thinking signatures, native or
//! wrapped chronological system updates, cache_control with the 4-breakpoint cap, tool choice
//! and generation fields.
//! Scope: pure request transform.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/cache_policy.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/tool_schema.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::to_value
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/anthropic/mod.rs
//! Invariants: breakpoints are spent tools -> system -> messages and drop silently past 4;
//! key order matches the baseline body schema field order.
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

use std::collections::HashSet;

use opencode_llm_api::{
    CacheHint, ContentPart, LlmError, LlmRequest, MediaData, Message, MessageRole, ToolContent,
    ToolResultPart, ToolResultValue,
};
use serde_json::Value;

use super::wire::{Choice, Obj, match_tool_choice, meta_str, num};
use crate::cache_policy::{Breakpoints, ttl_bucket};
use crate::shared::{
    IMAGE_MIMES, system_update_text, tool_result_text, unsupported_content, validate_media,
    wrap_system_update,
};
use crate::tool_schema::model_compatibility;

pub(crate) const ROUTE: &str = "Anthropic Messages";
const BREAKPOINT_CAP: u32 = 4;
const DEFAULT_MAX_TOKENS: f64 = 4096.0;

/// `fromRequest`.
pub(crate) fn body(request: &LlmRequest) -> Result<Value, LlmError> {
    let tool_choice = match &request.tool_choice {
        Some(choice) => lower_tool_choice(choice)?,
        None => None,
    };
    let generation = request.generation.clone().unwrap_or_default();
    let mut breakpoints = Breakpoints::new(BREAKPOINT_CAP);
    let tools_disabled = request.tools.is_empty()
        || request
            .tool_choice
            .as_ref()
            .is_some_and(|c| c.kind == opencode_llm_api::ToolChoiceType::None);
    let compatibility = request
        .model
        .compatibility
        .as_ref()
        .and_then(|c| c.tool_schema);
    let tools = (!tools_disabled).then(|| {
        Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    Obj::new()
                        .set("name", tool.name.clone())
                        .set("description", tool.description.clone())
                        .set(
                            "input_schema",
                            Value::Object(model_compatibility(&tool.input_schema, compatibility)),
                        )
                        .opt(
                            "cache_control",
                            cache_control(&mut breakpoints, tool.cache.as_ref()),
                        )
                        .build()
                })
                .collect(),
        )
    });
    let system = (!request.system.is_empty()).then(|| {
        Value::Array(
            request
                .system
                .iter()
                .map(|part| text_block(&mut breakpoints, &part.text, part.cache.as_ref()))
                .collect(),
        )
    });
    let messages = lower_messages(request, &mut breakpoints)?;
    if breakpoints.dropped > 0 {
        tracing::warn!(
            "Anthropic Messages: dropped {} cache breakpoint(s); the API allows at most {BREAKPOINT_CAP} per request.",
            breakpoints.dropped
        );
    }
    let output_limit = request
        .model
        .defaults
        .as_ref()
        .and_then(|d| d.limits.as_ref())
        .and_then(|l| l.output)
        .or_else(|| {
            request
                .model
                .route
                .defaults()
                .limits
                .as_ref()
                .and_then(|l| l.output)
        })
        .map_or(DEFAULT_MAX_TOKENS, |v| v as f64);
    Ok(Obj::new()
        .set("model", request.model.id.as_str())
        .opt("system", system)
        .set("messages", Value::Array(messages))
        .opt("tools", tools)
        .opt("tool_choice", tool_choice)
        .set("stream", true)
        .set(
            "max_tokens",
            num(generation.max_tokens.unwrap_or(output_limit)),
        )
        .opt("temperature", generation.temperature.map(num))
        .opt("top_p", generation.top_p.map(num))
        .opt("top_k", generation.top_k.map(num))
        .opt(
            "stop_sequences",
            generation
                .stop
                .map(|s| Value::Array(s.into_iter().map(Value::String).collect())),
        )
        .opt("thinking", lower_thinking(request)?)
        .build())
}

/// `cacheControl`: consume a breakpoint, `ttl: "1h"` for hints of an hour or more.
fn cache_control(breakpoints: &mut Breakpoints, cache: Option<&CacheHint>) -> Option<Value> {
    let cache = cache?;
    if !breakpoints.take() {
        return None;
    }
    Some(
        Obj::new()
            .set("type", "ephemeral")
            .opt("ttl", ttl_bucket(cache.ttl_seconds))
            .build(),
    )
}

fn text_block(breakpoints: &mut Breakpoints, text: &str, cache: Option<&CacheHint>) -> Value {
    Obj::new()
        .set("type", "text")
        .set("text", text)
        .opt("cache_control", cache_control(breakpoints, cache))
        .build()
}

fn lower_tool_choice(choice: &opencode_llm_api::ToolChoice) -> Result<Option<Value>, LlmError> {
    Ok(match match_tool_choice(ROUTE, choice)? {
        Choice::Auto => Some(Obj::new().set("type", "auto").build()),
        Choice::None => None,
        Choice::Required => Some(Obj::new().set("type", "any").build()),
        Choice::Tool(name) => Some(Obj::new().set("type", "tool").set("name", name).build()),
    })
}

fn image_block(media_type: &str, data: &MediaData) -> Result<Value, LlmError> {
    let media = validate_media(ROUTE, media_type, data, IMAGE_MIMES)?;
    Ok(Obj::new()
        .set("type", "image")
        .set(
            "source",
            Obj::new()
                .set("type", "base64")
                .set("media_type", media.mime)
                .set("data", media.base64)
                .build(),
        )
        .build())
}

/// Server tool result wire type by tool name.
fn server_tool_result_type(name: &str) -> Option<&'static str> {
    match name {
        "web_search" => Some("web_search_tool_result"),
        "code_execution" => Some("code_execution_tool_result"),
        "web_fetch" => Some("web_fetch_tool_result"),
        _ => None,
    }
}

/// `part.result.value` as raw JSON.
fn result_value(result: &ToolResultValue) -> Value {
    match result {
        ToolResultValue::Json(v) | ToolResultValue::Text(v) | ToolResultValue::Error(v) => {
            v.clone()
        }
        ToolResultValue::Content(c) => serde_json::to_value(c).unwrap_or(Value::Null),
    }
}

fn lower_server_tool_result(part: &ToolResultPart) -> Result<Value, LlmError> {
    let Some(kind) = server_tool_result_type(&part.name) else {
        return Err(LlmError::invalid_request(format!(
            "Anthropic Messages does not know how to round-trip server tool result for {}",
            part.name
        )));
    };
    Ok(Obj::new()
        .set("type", kind)
        .set("tool_use_id", part.id.clone())
        .set("content", result_value(&part.result))
        .build())
}

/// Text/json/error results stay a string; `content` results become text/image blocks.
fn lower_tool_result_content(part: &ToolResultPart) -> Result<Value, LlmError> {
    let ToolResultValue::Content(items) = &part.result else {
        return Ok(Value::String(tool_result_text(part)));
    };
    items
        .iter()
        .map(|item| match item {
            ToolContent::Text { text } => Ok(Obj::new()
                .set("type", "text")
                .set("text", text.clone())
                .build()),
            ToolContent::File { uri, mime, .. } => image_block(mime, &MediaData::Text(uri.clone())),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// Native chronological system updates are a Claude Opus 4.8 feature only.
fn supports_native_system_updates(request: &LlmRequest) -> bool {
    request.model.id.as_str() == "claude-opus-4-8"
}

fn ends_in_server_tool_use(message: &Message) -> bool {
    message.role == MessageRole::Assistant
        && matches!(message.content.last(), Some(ContentPart::ToolCall(c)) if c.provider_executed == Some(true))
}

fn can_use_native_system_update(messages: &[Message], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).and_then(|i| messages.get(i)) else {
        return false;
    };
    let next = messages.get(index + 1);
    previous.role != MessageRole::System
        && (matches!(previous.role, MessageRole::User | MessageRole::Tool)
            || ends_in_server_tool_use(previous))
        && next.is_none_or(|n| n.role == MessageRole::Assistant)
}

fn splits_local_tool_results(messages: &[Message], index: usize) -> bool {
    let mut pending = HashSet::new();
    for message in &messages[..index] {
        for part in &message.content {
            match part {
                ContentPart::ToolCall(call)
                    if message.role == MessageRole::Assistant
                        && call.provider_executed != Some(true) =>
                {
                    pending.insert(call.id.as_str());
                }
                ContentPart::ToolResult(result) if message.role == MessageRole::Tool => {
                    pending.remove(result.id.as_str());
                }
                _ => {}
            }
        }
    }
    !pending.is_empty()
}

/// `systemUpdateText` texts plus the cache hint of the last part.
fn system_update(message: &Message) -> Result<(Vec<String>, Option<&CacheHint>), LlmError> {
    let texts = system_update_text(ROUTE, message)?;
    let cache = match message.content.last() {
        Some(ContentPart::Text { cache, .. }) => cache.as_ref(),
        _ => None,
    };
    Ok((texts, cache))
}

fn lower_messages(
    request: &LlmRequest,
    breakpoints: &mut Breakpoints,
) -> Result<Vec<Value>, LlmError> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for (index, message) in request.messages.iter().enumerate() {
        match message.role {
            MessageRole::System => {
                if splits_local_tool_results(&request.messages, index) {
                    return Err(LlmError::invalid_request(
                        "Anthropic Messages system updates cannot split a local tool call from its tool result",
                    ));
                }
                if supports_native_system_updates(request)
                    && can_use_native_system_update(&request.messages, index)
                {
                    system_update_text(ROUTE, message)?;
                    let content = message
                        .content
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text, cache, .. } => {
                                Some(text_block(breakpoints, text, cache.as_ref()))
                            }
                            _ => None,
                        })
                        .collect();
                    out.push(("system", content));
                    continue;
                }
                let (texts, cache) = system_update(message)?;
                let block = text_block(breakpoints, &wrap_system_update(&texts.join("\n")), cache);
                match out.last_mut() {
                    Some(("user", content)) => content.push(block),
                    _ => out.push(("user", vec![block])),
                }
            }
            MessageRole::User => {
                let mut content = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text, cache, .. } => {
                            content.push(text_block(breakpoints, text, cache.as_ref()))
                        }
                        ContentPart::Media {
                            media_type, data, ..
                        } => content.push(image_block(media_type, data)?),
                        _ => return Err(unsupported_content(ROUTE, "user", &["text", "media"])),
                    }
                }
                out.push(("user", content));
            }
            MessageRole::Assistant => {
                let mut content = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text, cache, .. } => {
                            content.push(text_block(breakpoints, text, cache.as_ref()))
                        }
                        ContentPart::Reasoning {
                            text,
                            encrypted,
                            provider_metadata,
                            ..
                        } => content.push(
                            Obj::new()
                                .set("type", "thinking")
                                .set("thinking", text.clone())
                                .opt(
                                    "signature",
                                    encrypted.clone().or_else(|| {
                                        meta_str(
                                            provider_metadata.as_ref(),
                                            "anthropic",
                                            "signature",
                                        )
                                    }),
                                )
                                .build(),
                        ),
                        ContentPart::ToolCall(call) => content.push(
                            Obj::new()
                                .set(
                                    "type",
                                    if call.provider_executed == Some(true) {
                                        "server_tool_use"
                                    } else {
                                        "tool_use"
                                    },
                                )
                                .set("id", call.id.clone())
                                .set("name", call.name.clone())
                                .set("input", call.input.clone())
                                .build(),
                        ),
                        ContentPart::ToolResult(result)
                            if result.provider_executed == Some(true) =>
                        {
                            content.push(lower_server_tool_result(result)?)
                        }
                        _ => {
                            return Err(LlmError::invalid_request(
                                "Anthropic Messages assistant messages only support text, reasoning, and tool-call content for now",
                            ));
                        }
                    }
                }
                out.push(("assistant", content));
            }
            MessageRole::Tool => {
                let mut content = Vec::new();
                for part in &message.content {
                    let ContentPart::ToolResult(result) = part else {
                        return Err(unsupported_content(ROUTE, "tool", &["tool-result"]));
                    };
                    content.push(
                        Obj::new()
                            .set("type", "tool_result")
                            .set("tool_use_id", result.id.clone())
                            .set("content", lower_tool_result_content(result)?)
                            .opt(
                                "is_error",
                                matches!(result.result, ToolResultValue::Error(_)).then_some(true),
                            )
                            .opt(
                                "cache_control",
                                cache_control(breakpoints, result.cache.as_ref()),
                            )
                            .build(),
                    );
                }
                out.push(("user", content));
            }
        }
    }
    Ok(out
        .into_iter()
        .map(|(role, content)| {
            Obj::new()
                .set("role", role)
                .set("content", Value::Array(content))
                .build()
        })
        .collect())
}

/// `providerOptions.anthropic.thinking` -> `{ type: "enabled", budget_tokens }`.
fn lower_thinking(request: &LlmRequest) -> Result<Option<Value>, LlmError> {
    let Some(thinking) = request
        .provider_options
        .as_ref()
        .and_then(|o| o.get("anthropic"))
        .and_then(|a| a.get("thinking"))
        .and_then(Value::as_object)
    else {
        return Ok(None);
    };
    if thinking.get("type").and_then(Value::as_str) != Some("enabled") {
        return Ok(None);
    }
    let budget = thinking
        .get("budgetTokens")
        .and_then(Value::as_f64)
        .or_else(|| thinking.get("budget_tokens").and_then(Value::as_f64));
    let Some(budget) = budget else {
        return Err(LlmError::invalid_request(
            "Anthropic thinking provider option requires budgetTokens",
        ));
    };
    Ok(Some(
        Obj::new()
            .set("type", "enabled")
            .set("budget_tokens", num(budget))
            .build(),
    ))
}
