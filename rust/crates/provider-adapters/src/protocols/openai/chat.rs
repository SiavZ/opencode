//! Purpose: OpenAI Chat Completions request lowering (packages/llm/src/protocols/openai-chat.ts
//! `fromRequest`, `lowerMessages`, `lowerOptions`) and the `openai-chat` `Protocol`.
//! Scope: body construction. Stream parsing lives in chat_stream.rs.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/openai/chat_stream.rs
//! |-- provider-adapters/src/protocols/openai/options.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! `-- provider-adapters/src/shared.rs
//! External:
//! |-- options::is_openai_reasoning_effort
//! |-- options::reasoning_effort
//! |-- options::store
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/chat_stream.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! `-- runtime/src/execution_support.rs
//! Invariants: body keys follow the baseline body schema order (model, messages, tools,
//! tool_choice, stream, stream_options, store, reasoning_effort, max_tokens, temperature,
//! top_p, frequency_penalty, presence_penalty, seed, stop) so encoded bytes match recordings.
//! Tool-result images are deferred into one user vision message after the tool messages and
//! merged with a following system update.
//! Contract tests: provider-adapters/tests/openai_chat.rs

use opencode_llm_api::{
    ContentPart, GenerationOptions, LlmError, LlmRequest, MediaData, Message, MessageRole,
    ToolContent, ToolResultValue,
};
use serde_json::{Map, Value, json};

use crate::protocol::{Protocol, ProtocolState};
use crate::protocols::openai::chat_stream::ChatState;
use crate::protocols::openai::options;
use crate::protocols::openai::wire::{
    encode_json, js_num, lower_tool_choice, put, tool_parameters,
};
use crate::shared::{
    IMAGE_MIMES, join_text, system_update_text, tool_result_text, unsupported_content,
    validate_media, wrap_system_update,
};

pub const ADAPTER: &str = "openai-chat";
const ROUTE: &str = "OpenAI Chat";

/// `OpenAIChat.protocol`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAIChatProtocol;

impl Protocol for OpenAIChatProtocol {
    fn id(&self) -> &'static str {
        ADAPTER
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        from_request(request)
    }

    fn initial(&self, _request: &LlmRequest) -> Box<dyn ProtocolState> {
        Box::new(ChatState::default())
    }
}

/// `fromRequest`.
pub fn from_request(request: &LlmRequest) -> Result<Value, LlmError> {
    let mut body = Map::new();
    body.insert("model".into(), Value::from(request.model.id.as_str()));
    body.insert("messages".into(), Value::Array(lower_messages(request)?));
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool_parameters(request, &tool.input_schema),
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = &request.tool_choice {
        let lowered = lower_tool_choice(
            ROUTE,
            choice,
            |name| json!({ "type": "function", "function": { "name": name } }),
        )?;
        body.insert("tool_choice".into(), lowered);
    }
    body.insert("stream".into(), Value::Bool(true));
    body.insert("stream_options".into(), json!({ "include_usage": true }));
    let store = options::store(request);
    put(&mut body, "store", store.map(Value::Bool));
    if let Some(effort) = options::reasoning_effort(request) {
        if !options::is_openai_reasoning_effort(effort) {
            return Err(LlmError::invalid_request(format!(
                "OpenAI Chat does not support reasoning effort {effort}"
            )));
        }
        body.insert("reasoning_effort".into(), Value::from(effort));
    }
    lower_generation(&mut body, request.generation.as_ref());
    Ok(Value::Object(body))
}

fn lower_generation(body: &mut Map<String, Value>, generation: Option<&GenerationOptions>) {
    let Some(g) = generation else { return };
    put(body, "max_tokens", g.max_tokens.map(js_num));
    put(body, "temperature", g.temperature.map(js_num));
    put(body, "top_p", g.top_p.map(js_num));
    put(body, "frequency_penalty", g.frequency_penalty.map(js_num));
    put(body, "presence_penalty", g.presence_penalty.map(js_num));
    put(body, "seed", g.seed.map(js_num));
    put(body, "stop", g.stop.as_ref().map(|s| json!(s)));
}

fn image_part(media_type: &str, data: &MediaData) -> Result<Value, LlmError> {
    let media = validate_media(ROUTE, media_type, data, IMAGE_MIMES)?;
    Ok(json!({ "type": "image_url", "image_url": { "url": media.data_url } }))
}

fn lower_user(message: &Message) -> Result<Value, LlmError> {
    let mut content = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text, .. } => content.push(json!({ "type": "text", "text": text })),
            ContentPart::Media {
                media_type, data, ..
            } => content.push(image_part(media_type, data)?),
            _ => return Err(unsupported_content(ROUTE, "user", &["text", "media"])),
        }
    }
    if content.iter().all(|p| p["type"] == "text") {
        let text: String = content.iter().filter_map(|p| p["text"].as_str()).collect();
        return Ok(json!({ "role": "user", "content": text }));
    }
    Ok(json!({ "role": "user", "content": content }))
}

fn lower_assistant(message: &Message) -> Result<Value, LlmError> {
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text: t, .. } => text.push(t.as_str()),
            ContentPart::Reasoning { text: t, .. } => reasoning.push(t.as_str()),
            ContentPart::ToolCall(call) => tool_calls.push(json!({
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": encode_json(&call.input) },
            })),
            _ => {
                return Err(unsupported_content(
                    ROUTE,
                    "assistant",
                    &["text", "reasoning", "tool-call"],
                ));
            }
        }
    }
    let mut out = Map::new();
    out.insert("role".into(), Value::from("assistant"));
    let content = (!text.is_empty()).then(|| Value::from(join_text(text)));
    out.insert("content".into(), content.unwrap_or(Value::Null));
    if !tool_calls.is_empty() {
        out.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    let reasoning_content = if reasoning.is_empty() {
        message
            .native
            .as_ref()
            .and_then(|n| n.get("openaiCompatible"))
            .and_then(|n| n.get("reasoning_content"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        Some(reasoning.concat())
    };
    put(
        &mut out,
        "reasoning_content",
        reasoning_content.map(Value::from),
    );
    Ok(Value::Object(out))
}

/// `lowerToolMessages`: one `tool` message per result plus deferred vision parts.
fn lower_tool(message: &Message) -> Result<(Vec<Value>, Vec<Value>), LlmError> {
    let mut messages = Vec::new();
    let mut images = Vec::new();
    for part in &message.content {
        let ContentPart::ToolResult(result) = part else {
            return Err(unsupported_content(ROUTE, "tool", &["tool-result"]));
        };
        let ToolResultValue::Content(items) = &result.result else {
            messages.push(tool_message(&result.id, tool_result_text(result)));
            continue;
        };
        let text: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                ToolContent::Text { text } => Some(text.as_str()),
                ToolContent::File { .. } => None,
            })
            .collect();
        messages.push(tool_message(&result.id, text.join("\n")));
        for item in items {
            if let ToolContent::File { uri, mime, .. } = item {
                images.push(image_part(mime, &MediaData::Text(uri.clone()))?);
            }
        }
    }
    Ok((messages, images))
}

fn tool_message(id: &str, content: String) -> Value {
    json!({ "role": "tool", "tool_call_id": id, "content": content })
}

/// `lowerMessages`.
fn lower_messages(request: &LlmRequest) -> Result<Vec<Value>, LlmError> {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        let text = join_text(request.system.iter().map(|p| p.text.as_str()));
        messages.push(json!({ "role": "system", "content": text }));
    }
    let mut pending: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                let update = wrap_system_update(&join_text(
                    system_update_text(ROUTE, message)?
                        .iter()
                        .map(String::as_str),
                ));
                let text_part = json!({ "type": "text", "text": update });
                if !pending.is_empty() {
                    let mut content = std::mem::take(&mut pending);
                    content.push(text_part);
                    messages.push(json!({ "role": "user", "content": content }));
                    continue;
                }
                match messages.last_mut() {
                    Some(prev) if prev["role"] == "user" && prev["content"].is_string() => {
                        let joined =
                            format!("{}\n{update}", prev["content"].as_str().unwrap_or(""));
                        *prev = json!({ "role": "user", "content": joined });
                    }
                    Some(prev) if prev["role"] == "user" && prev["content"].is_array() => {
                        if let Some(parts) = prev["content"].as_array_mut() {
                            parts.push(text_part);
                        }
                    }
                    _ => messages.push(json!({ "role": "user", "content": update })),
                }
            }
            MessageRole::Tool => {
                let (tool_messages, images) = lower_tool(message)?;
                messages.extend(tool_messages);
                pending.extend(images);
            }
            MessageRole::User | MessageRole::Assistant => {
                flush(&mut messages, &mut pending);
                messages.push(if message.role == MessageRole::User {
                    lower_user(message)?
                } else {
                    lower_assistant(message)?
                });
            }
        }
    }
    flush(&mut messages, &mut pending);
    Ok(messages)
}

fn flush(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
    }
}
