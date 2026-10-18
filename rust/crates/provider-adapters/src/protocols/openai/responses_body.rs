//! Purpose: OpenAI Responses request lowering (packages/llm/src/protocols/openai-responses.ts
//! `fromRequest`, `lowerMessages`, `lowerOptions`) plus the WebSocket `response.create`
//! message (`webSocketMessage`).
//! Scope: body construction for HTTP and WebSocket transports.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/options.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! `-- provider-adapters/src/shared.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! `-- provider-adapters/src/protocols/openai/responses_stream.rs
//! Invariants: body keys follow the baseline encode order (model, input, instructions, tools,
//! tool_choice, store, service_tier, prompt_cache_key, include, reasoning, text,
//! max_output_tokens, temperature, top_p, stream). With `store !== false` reasoning and hosted
//! tool results replay as deduplicated `item_reference`s; with `store: false` reasoning items
//! for one id are merged and dropped unless they carry encrypted state.
//! Contract tests: provider-adapters/tests/openai_responses.rs

use opencode_llm_api::{
    ContentPart, LlmError, LlmRequest, MediaData, MessageRole, ToolContent, ToolResultPart,
    ToolResultValue,
};
use serde_json::{Map, Value, json};

use crate::protocols::openai::options;
use crate::protocols::openai::wire::{
    encode_json, js_num, lower_tool_choice, put, tool_parameters,
};
use crate::shared::{
    IMAGE_MIMES, join_text, system_update_text, tool_result_text, unsupported_content,
    validate_media, wrap_system_update,
};

const ROUTE: &str = "OpenAI Responses";

/// `fromRequest`: the HTTP body (`stream: true`).
pub fn from_request(request: &LlmRequest) -> Result<Value, LlmError> {
    let opts = lower_options(request)?;
    let mut core = Map::new();
    core.insert("model".into(), Value::from(request.model.id.as_str()));
    core.insert("input".into(), Value::Array(lower_messages(request)?));
    put(&mut core, "instructions", opts.instructions);
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool_parameters(request, &tool.input_schema),
                    "strict": false,
                })
            })
            .collect();
        core.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = &request.tool_choice {
        let lowered = lower_tool_choice(
            ROUTE,
            choice,
            |name| json!({ "type": "function", "name": name }),
        )?;
        core.insert("tool_choice".into(), lowered);
    }
    put(&mut core, "store", opts.store);
    put(&mut core, "service_tier", opts.service_tier);
    put(&mut core, "prompt_cache_key", opts.prompt_cache_key);
    put(&mut core, "include", opts.include);
    put(&mut core, "reasoning", opts.reasoning);
    put(&mut core, "text", opts.text);
    if let Some(g) = &request.generation {
        put(&mut core, "max_output_tokens", g.max_tokens.map(js_num));
        put(&mut core, "temperature", g.temperature.map(js_num));
        put(&mut core, "top_p", g.top_p.map(js_num));
    }
    core.insert("stream".into(), Value::Bool(true));
    Ok(Value::Object(core))
}

/// `webSocketMessage` + `encodeWebSocketMessage`: drop `stream`, tag `response.create`, and
/// encode with `type` first, then the core schema keys, then any overlay keys.
pub fn web_socket_message(body: Value) -> Result<String, LlmError> {
    let Value::Object(mut body) = body else {
        return Err(LlmError::invalid_request(
            "OpenAI Responses WebSocket body must be a JSON object",
        ));
    };
    body.shift_remove("stream");
    let mut message = Map::new();
    message.insert("type".into(), Value::from("response.create"));
    for key in CORE_KEYS {
        if let Some(value) = body.shift_remove(*key) {
            message.insert((*key).to_owned(), value);
        }
    }
    if !message.get("model").is_some_and(Value::is_string)
        || !message.get("input").is_some_and(Value::is_array)
    {
        return Err(LlmError::invalid_request(
            "OpenAI Responses WebSocket message requires model and input",
        ));
    }
    message.extend(body);
    Ok(encode_json(&Value::Object(message)))
}

const CORE_KEYS: &[&str] = &[
    "model",
    "input",
    "instructions",
    "tools",
    "tool_choice",
    "store",
    "service_tier",
    "prompt_cache_key",
    "include",
    "reasoning",
    "text",
    "max_output_tokens",
    "temperature",
    "top_p",
];

#[derive(Default)]
struct LoweredOptions {
    instructions: Option<Value>,
    store: Option<Value>,
    prompt_cache_key: Option<Value>,
    include: Option<Value>,
    reasoning: Option<Value>,
    text: Option<Value>,
    service_tier: Option<Value>,
}

fn lower_options(request: &LlmRequest) -> Result<LoweredOptions, LlmError> {
    let effort = options::reasoning_effort(request);
    if let Some(effort) = effort.filter(|e| !options::is_openai_reasoning_effort(e)) {
        return Err(LlmError::invalid_request(format!(
            "OpenAI Responses does not support reasoning effort {effort}"
        )));
    }
    let summary = options::reasoning_summary(request);
    let reasoning = (effort.is_some() || summary.is_some()).then(|| {
        let mut r = Map::new();
        put(&mut r, "effort", effort.map(Value::from));
        put(&mut r, "summary", summary.map(Value::from));
        Value::Object(r)
    });
    Ok(LoweredOptions {
        instructions: options::instructions(request)
            .filter(|s| !s.is_empty())
            .map(Value::from),
        store: options::store(request).map(Value::Bool),
        prompt_cache_key: options::prompt_cache_key(request)
            .filter(|s| !s.is_empty())
            .map(Value::from),
        include: options::include(request).map(|v| json!(v)),
        reasoning,
        text: options::text_verbosity(request).map(|v| json!({ "verbosity": v })),
        service_tier: options::service_tier(request).map(Value::from),
    })
}

fn image(media_type: &str, data: &MediaData) -> Result<Value, LlmError> {
    let media = validate_media(ROUTE, media_type, data, IMAGE_MIMES)?;
    Ok(json!({ "type": "input_image", "image_url": media.data_url }))
}

fn openai_item_id(metadata: Option<&opencode_llm_api::ProviderMetadata>) -> Option<&str> {
    metadata?
        .get("openai")?
        .get("itemId")?
        .as_str()
        .filter(|s| !s.is_empty())
}

/// `lowerReasoning`: `(id, summary, encrypted_content)` where encrypted is absent, null or text.
fn lower_reasoning(part: &ContentPart) -> Option<(String, Vec<Value>, Option<Value>)> {
    let ContentPart::Reasoning {
        text,
        provider_metadata,
        ..
    } = part
    else {
        return None;
    };
    let id = openai_item_id(provider_metadata.as_ref())?;
    let encrypted = provider_metadata
        .as_ref()
        .and_then(|m| m.get("openai"))
        .and_then(|m| m.get("reasoningEncryptedContent"))
        .filter(|v| v.is_string() || v.is_null())
        .cloned();
    let summary = if text.is_empty() {
        vec![]
    } else {
        vec![json!({ "type": "summary_text", "text": text })]
    };
    Some((id.to_owned(), summary, encrypted))
}

fn lower_tool_output(part: &ToolResultPart) -> Result<Value, LlmError> {
    let ToolResultValue::Content(items) = &part.result else {
        return Ok(Value::from(tool_result_text(part)));
    };
    items
        .iter()
        .map(|item| match item {
            ToolContent::Text { text } => Ok(json!({ "type": "input_text", "text": text })),
            ToolContent::File { uri, mime, .. } => image(mime, &MediaData::Text(uri.clone())),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn is_user(item: &Value) -> bool {
    item.get("role") == Some(&Value::from("user"))
}

/// `lowerMessages`.
fn lower_messages(request: &LlmRequest) -> Result<Vec<Value>, LlmError> {
    let mut input: Vec<Value> = Vec::new();
    if !request.system.is_empty() {
        let text = join_text(request.system.iter().map(|p| p.text.as_str()));
        input.push(json!({ "role": "system", "content": text }));
    }
    let store = options::store(request);
    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                let text = wrap_system_update(&join_text(
                    system_update_text(ROUTE, message)?
                        .iter()
                        .map(String::as_str),
                ));
                let part = json!({ "type": "input_text", "text": text });
                match input.last_mut() {
                    Some(prev) if is_user(prev) => {
                        if let Some(content) = prev["content"].as_array_mut() {
                            content.push(part);
                        }
                    }
                    _ => input.push(json!({ "role": "user", "content": [part] })),
                }
            }
            MessageRole::User => {
                let content = message
                    .content
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text, .. } => {
                            Ok(json!({ "type": "input_text", "text": text }))
                        }
                        ContentPart::Media {
                            media_type, data, ..
                        } => image(media_type, data),
                        _ => Err(unsupported_content(ROUTE, "user", &["text", "media"])),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                input.push(json!({ "role": "user", "content": content }));
            }
            MessageRole::Assistant => lower_assistant(&mut input, message, store)?,
            MessageRole::Tool => {
                for part in &message.content {
                    let ContentPart::ToolResult(result) = part else {
                        return Err(unsupported_content(ROUTE, "tool", &["tool-result"]));
                    };
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": result.id,
                        "output": lower_tool_output(result)?,
                    }));
                }
            }
        }
    }
    // With store:false, OpenAI only accepts previous reasoning items when the complete item has
    // encrypted state. Summary blocks for one item may carry that state only on the last block,
    // so filter after they have been joined.
    if store == Some(false) {
        input.retain(|item| {
            item.get("type") != Some(&Value::from("reasoning"))
                || item.get("encrypted_content").is_some_and(Value::is_string)
        });
    }
    Ok(input)
}

fn lower_assistant(
    input: &mut Vec<Value>,
    message: &opencode_llm_api::Message,
    store: Option<bool>,
) -> Result<(), LlmError> {
    let mut text: Vec<&str> = Vec::new();
    // Replay items by reasoning id: index into `input` so later blocks fold into the same item.
    let mut reasoning_items: Vec<(String, usize)> = Vec::new();
    let mut reasoning_refs: Vec<String> = Vec::new();
    let mut hosted_refs: Vec<String> = Vec::new();
    let flush = |input: &mut Vec<Value>, text: &mut Vec<&str>| {
        if text.is_empty() {
            return;
        }
        let content: Vec<Value> = text
            .drain(..)
            .map(|t| json!({ "type": "output_text", "text": t }))
            .collect();
        input.push(json!({ "role": "assistant", "content": content }));
    };
    for part in &message.content {
        match part {
            ContentPart::Text { text: t, .. } => text.push(t),
            ContentPart::Reasoning { .. } => {
                flush(input, &mut text);
                let Some((id, summary, encrypted)) = lower_reasoning(part) else {
                    continue;
                };
                if store != Some(false) {
                    if !reasoning_refs.contains(&id) {
                        input.push(json!({ "type": "item_reference", "id": id }));
                        reasoning_refs.push(id);
                    }
                    continue;
                }
                if let Some((_, index)) = reasoning_items.iter().find(|(rid, _)| *rid == id) {
                    let item = &mut input[*index];
                    if let Some(existing) = item["summary"].as_array_mut() {
                        existing.extend(summary);
                    }
                    if let Some(Value::String(state)) = encrypted {
                        item["encrypted_content"] = Value::String(state);
                    }
                    continue;
                }
                let mut replay = Map::new();
                replay.insert("type".into(), Value::from("reasoning"));
                replay.insert("summary".into(), Value::Array(summary));
                put(&mut replay, "encrypted_content", encrypted);
                reasoning_items.push((id, input.len()));
                input.push(Value::Object(replay));
            }
            ContentPart::ToolCall(call) => {
                flush(input, &mut text);
                if call.provider_executed == Some(true) {
                    continue;
                }
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": encode_json(&call.input),
                }));
            }
            ContentPart::ToolResult(result) if result.provider_executed == Some(true) => {
                flush(input, &mut text);
                let Some(item_id) = openai_item_id(result.provider_metadata.as_ref()) else {
                    continue;
                };
                if store != Some(false) && !hosted_refs.iter().any(|r| r == item_id) {
                    input.push(json!({ "type": "item_reference", "id": item_id }));
                }
                hosted_refs.push(item_id.to_owned());
            }
            _ => {
                return Err(unsupported_content(
                    ROUTE,
                    "assistant",
                    &["text", "reasoning", "tool-call", "tool-result"],
                ));
            }
        }
    }
    flush(input, &mut text);
    Ok(())
}
