//! Purpose: lower an option-resolved `LlmRequest` into the Gemini `streamGenerateContent` body
//! (packages/llm/src/protocols/gemini.ts `fromRequest`): contents with user/model roles, inline
//! media, function calls with thought signatures, function responses (text and image tool
//! results), wrapped chronological system updates, system instruction, tool declarations with
//! the Gemini schema projection, tool config and generation config with thinkingConfig.
//! Scope: pure request transform.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/tool_schema.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/gemini/mod.rs
//! Invariants: Gemini ignores cache hints (implicit caching); tools and toolConfig are omitted
//! when tool choice is `none`; generationConfig is omitted when every field is unset.
//! Contract tests: provider-adapters/tests/gemini.rs

use opencode_llm_api::{
    ContentPart, LlmError, LlmRequest, MediaData, MessageRole, ToolChoiceType, ToolContent,
    ToolResultValue,
};
use serde_json::{Map, Value};

use crate::protocols::anthropic::wire::{Choice, Obj, match_tool_choice, meta_str, num};
use crate::shared::{
    AUDIO_MIMES, IMAGE_MIMES, VIDEO_MIMES, join_text, system_update_text, tool_result_text,
    unsupported_content, validate_media, wrap_system_update,
};
use crate::tool_schema::{gemini as gemini_schema, model_compatibility};

const ROUTE: &str = "Gemini";

fn media_mimes() -> Vec<&'static str> {
    [IMAGE_MIMES, VIDEO_MIMES, AUDIO_MIMES].concat()
}

/// `fromRequest`.
pub(crate) fn body(request: &LlmRequest) -> Result<Value, LlmError> {
    let tools_enabled = !request.tools.is_empty()
        && request
            .tool_choice
            .as_ref()
            .is_none_or(|c| c.kind != ToolChoiceType::None);
    let generation = request.generation.clone().unwrap_or_default();
    let compatibility = request
        .model
        .compatibility
        .as_ref()
        .and_then(|c| c.tool_schema);
    let generation_config = Obj::new()
        .opt("maxOutputTokens", generation.max_tokens.map(num))
        .opt("temperature", generation.temperature.map(num))
        .opt("topP", generation.top_p.map(num))
        .opt("topK", generation.top_k.map(num))
        .opt(
            "stopSequences",
            generation
                .stop
                .map(|s| Value::Array(s.into_iter().map(Value::String).collect())),
        )
        .opt("thinkingConfig", thinking_config(request));
    let tools = tools_enabled.then(|| {
        let declarations = request
            .tools
            .iter()
            .map(|tool| {
                let schema = model_compatibility(&tool.input_schema, compatibility);
                let parameters = gemini_schema(&schema);
                Obj::new()
                    .set("name", tool.name.clone())
                    .set("description", tool.description.clone())
                    // `convert` yields undefined for empty object schemas.
                    .opt(
                        "parameters",
                        (!parameters.is_empty() || !is_empty_object(&schema))
                            .then_some(Value::Object(parameters)),
                    )
                    .build()
            })
            .collect();
        Value::Array(vec![
            Obj::new()
                .set("functionDeclarations", Value::Array(declarations))
                .build(),
        ])
    });
    let tool_config = match (&request.tool_choice, tools_enabled) {
        (Some(choice), true) => Some(lower_tool_config(choice)?),
        _ => None,
    };
    let system = (!request.system.is_empty()).then(|| {
        Obj::new()
            .set(
                "parts",
                Value::Array(vec![
                    Obj::new()
                        .set(
                            "text",
                            join_text(request.system.iter().map(|p| p.text.as_str())),
                        )
                        .build(),
                ]),
            )
            .build()
    });
    Ok(Obj::new()
        .set("contents", Value::Array(lower_messages(request)?))
        .opt("systemInstruction", system)
        .opt("tools", tools)
        .opt("toolConfig", tool_config)
        .opt(
            "generationConfig",
            (!generation_config.is_empty()).then(|| generation_config.build()),
        )
        .build())
}

/// `projectNode` returns `undefined` for an object schema without properties.
fn is_empty_object(schema: &Map<String, Value>) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        && schema
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(Map::is_empty)
        && schema
            .get("additionalProperties")
            .is_none_or(|v| matches!(v, Value::Bool(false) | Value::Null))
}

fn lower_tool_config(choice: &opencode_llm_api::ToolChoice) -> Result<Value, LlmError> {
    let config = |mode: &str, allowed: Option<&str>| {
        Obj::new()
            .set(
                "functionCallingConfig",
                Obj::new()
                    .set("mode", mode)
                    .opt(
                        "allowedFunctionNames",
                        allowed.map(|n| Value::Array(vec![Value::String(n.to_owned())])),
                    )
                    .build(),
            )
            .build()
    };
    Ok(match match_tool_choice(ROUTE, choice)? {
        Choice::Auto => config("AUTO", None),
        Choice::None => config("NONE", None),
        Choice::Required => config("ANY", None),
        Choice::Tool(name) => config("ANY", Some(name)),
    })
}

fn inline_data(media_type: &str, data: &MediaData) -> Result<Value, LlmError> {
    let media = validate_media(ROUTE, media_type, data, &media_mimes())?;
    Ok(Obj::new()
        .set(
            "inlineData",
            Obj::new()
                .set("mimeType", media.mime)
                .set("data", media.base64)
                .build(),
        )
        .build())
}

fn text_part(text: &str) -> Value {
    Obj::new().set("text", text).build()
}

fn function_response(name: &str, content: String) -> Value {
    Obj::new()
        .set(
            "functionResponse",
            Obj::new()
                .set("name", name)
                .set(
                    "response",
                    Obj::new().set("name", name).set("content", content).build(),
                )
                .build(),
        )
        .build()
}

fn lower_messages(request: &LlmRequest) -> Result<Vec<Value>, LlmError> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                let text = wrap_system_update(&system_update_text(ROUTE, message)?.join("\n"));
                match out.last_mut() {
                    Some(("user", parts)) => parts.push(text_part(&text)),
                    _ => out.push(("user", vec![text_part(&text)])),
                }
            }
            MessageRole::User => {
                let mut parts = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text, .. } => parts.push(text_part(text)),
                        ContentPart::Media {
                            media_type, data, ..
                        } => parts.push(inline_data(media_type, data)?),
                        _ => return Err(unsupported_content(ROUTE, "user", &["text", "media"])),
                    }
                }
                out.push(("user", parts));
            }
            MessageRole::Assistant => {
                let mut parts = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text, .. } => parts.push(text_part(text)),
                        ContentPart::Reasoning {
                            text,
                            provider_metadata,
                            ..
                        } => parts.push(
                            Obj::new()
                                .set("text", text.clone())
                                .set("thought", true)
                                .opt(
                                    "thoughtSignature",
                                    meta_str(
                                        provider_metadata.as_ref(),
                                        "google",
                                        "thoughtSignature",
                                    ),
                                )
                                .build(),
                        ),
                        ContentPart::ToolCall(call) => parts.push(
                            Obj::new()
                                .set(
                                    "functionCall",
                                    Obj::new()
                                        .set("name", call.name.clone())
                                        .set("args", call.input.clone())
                                        .build(),
                                )
                                .opt(
                                    "thoughtSignature",
                                    meta_str(
                                        call.provider_metadata.as_ref(),
                                        "google",
                                        "thoughtSignature",
                                    ),
                                )
                                .build(),
                        ),
                        _ => {
                            return Err(unsupported_content(
                                ROUTE,
                                "assistant",
                                &["text", "reasoning", "tool-call"],
                            ));
                        }
                    }
                }
                out.push(("model", parts));
            }
            MessageRole::Tool => {
                let mut parts = Vec::new();
                for part in &message.content {
                    let ContentPart::ToolResult(result) = part else {
                        return Err(unsupported_content(ROUTE, "tool", &["tool-result"]));
                    };
                    let ToolResultValue::Content(items) = &result.result else {
                        parts.push(function_response(&result.name, tool_result_text(result)));
                        continue;
                    };
                    let text: Vec<&str> = items
                        .iter()
                        .filter_map(|item| match item {
                            ToolContent::Text { text } => Some(text.as_str()),
                            ToolContent::File { .. } => None,
                        })
                        .collect();
                    parts.push(function_response(&result.name, text.join("\n")));
                    for item in items {
                        if let ToolContent::File { uri, mime, .. } = item {
                            parts.push(inline_data(mime, &MediaData::Text(uri.clone()))?);
                        }
                    }
                }
                out.push(("user", parts));
            }
        }
    }
    Ok(out
        .into_iter()
        .map(|(role, parts)| {
            Obj::new()
                .set("role", role)
                .set("parts", Value::Array(parts))
                .build()
        })
        .collect())
}

/// `providerOptions.gemini.thinkingConfig` (numbers and booleans only).
fn thinking_config(request: &LlmRequest) -> Option<Value> {
    let value = request
        .provider_options
        .as_ref()?
        .get("gemini")?
        .get("thinkingConfig")?
        .as_object()?;
    let config = Obj::new()
        .opt(
            "thinkingBudget",
            value.get("thinkingBudget").and_then(Value::as_f64).map(num),
        )
        .opt(
            "includeThoughts",
            value.get("includeThoughts").and_then(Value::as_bool),
        );
    (!config.is_empty()).then(|| config.build())
}
