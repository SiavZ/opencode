//! Purpose: lower an option-resolved `LlmRequest` into the Bedrock Converse body
//! (packages/llm/src/protocols/bedrock-converse.ts `fromRequest`, utils/bedrock-cache.ts,
//! utils/bedrock-media.ts): messages, image/document media, tool use/result (json, text, image),
//! signed reasoningContent, positional cachePoint markers with the 4-breakpoint cap and 1h ttl,
//! toolConfig with toolChoice, inferenceConfig and `additionalModelRequestFields.top_k`.
//! Scope: pure request transform.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/cache_policy.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/tool_schema.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/bedrock/mod.rs
//! Invariants: breakpoints are spent tools -> system -> messages; a cachePoint block follows the
//! block it marks; key order matches the baseline body schema field order.
//! Contract tests: provider-adapters/tests/bedrock_converse.rs

use opencode_llm_api::{
    CacheHint, ContentPart, LlmError, LlmRequest, MediaData, MessageRole, ToolChoiceType,
    ToolContent, ToolResultPart, ToolResultValue,
};
use serde_json::Value;

use crate::cache_policy::{Breakpoints, ttl_bucket};
use crate::protocols::anthropic::wire::{Choice, Obj, match_tool_choice, meta_str, num};
use crate::shared::{
    system_update_text, tool_result_text, unsupported_content, validate_media, wrap_system_update,
};
use crate::tool_schema::model_compatibility;

pub(crate) const ROUTE: &str = "Bedrock Converse";
pub(crate) const BREAKPOINT_CAP: u32 = 4;

const IMAGE_FORMATS: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpeg"),
    ("image/jpg", "jpeg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

const DOCUMENT_FORMATS: &[(&str, &str)] = &[
    ("application/pdf", "pdf"),
    ("text/csv", "csv"),
    ("application/msword", "doc"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    ("application/vnd.ms-excel", "xls"),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    ("text/html", "html"),
    ("text/plain", "txt"),
    ("text/markdown", "md"),
];

/// `fromRequest`.
pub(crate) fn body(request: &LlmRequest) -> Result<Value, LlmError> {
    let tool_choice = match &request.tool_choice {
        Some(choice) => lower_tool_choice(choice)?,
        None => None,
    };
    let generation = request.generation.clone().unwrap_or_default();
    let mut breakpoints = Breakpoints::new(BREAKPOINT_CAP);
    let tools_enabled = !request.tools.is_empty()
        && request
            .tool_choice
            .as_ref()
            .is_none_or(|c| c.kind != ToolChoiceType::None);
    let compatibility = request
        .model
        .compatibility
        .as_ref()
        .and_then(|c| c.tool_schema);
    let tool_config = tools_enabled.then(|| {
        let mut tools = Vec::new();
        for tool in &request.tools {
            tools.push(
                Obj::new()
                    .set(
                        "toolSpec",
                        Obj::new()
                            .set("name", tool.name.clone())
                            .set("description", tool.description.clone())
                            .set(
                                "inputSchema",
                                Obj::new()
                                    .set(
                                        "json",
                                        Value::Object(model_compatibility(
                                            &tool.input_schema,
                                            compatibility,
                                        )),
                                    )
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            );
            if let Some(point) = cache_point(&mut breakpoints, tool.cache.as_ref()) {
                tools.push(point);
            }
        }
        Obj::new()
            .set("tools", Value::Array(tools))
            .opt("toolChoice", tool_choice.clone())
            .build()
    });
    let system = (!request.system.is_empty()).then(|| {
        Value::Array(
            request
                .system
                .iter()
                .flat_map(|part| text_with_cache(&mut breakpoints, &part.text, part.cache.as_ref()))
                .collect(),
        )
    });
    let messages = lower_messages(request, &mut breakpoints)?;
    if breakpoints.dropped > 0 {
        tracing::warn!(
            "Bedrock Converse: dropped {} cache breakpoint(s); the API allows at most {BREAKPOINT_CAP} per request.",
            breakpoints.dropped
        );
    }
    let no_inference = generation.max_tokens.is_none()
        && generation.temperature.is_none()
        && generation.top_p.is_none()
        && generation.stop.as_ref().is_none_or(Vec::is_empty);
    let inference = (!no_inference).then(|| {
        Obj::new()
            .opt("maxTokens", generation.max_tokens.map(num))
            .opt("temperature", generation.temperature.map(num))
            .opt("topP", generation.top_p.map(num))
            .opt(
                "stopSequences",
                generation
                    .stop
                    .clone()
                    .map(|s| Value::Array(s.into_iter().map(Value::String).collect())),
            )
            .build()
    });
    Ok(Obj::new()
        .set("modelId", request.model.id.as_str())
        .set("messages", Value::Array(messages))
        .opt("system", system)
        .opt("inferenceConfig", inference)
        .opt("toolConfig", tool_config)
        .opt(
            "additionalModelRequestFields",
            generation
                .top_k
                .map(|k| Obj::new().set("top_k", num(k)).build()),
        )
        .build())
}

/// `BedrockCache.block`.
fn cache_point(breakpoints: &mut Breakpoints, cache: Option<&CacheHint>) -> Option<Value> {
    let cache = cache?;
    if !breakpoints.take() {
        return None;
    }
    Some(
        Obj::new()
            .set(
                "cachePoint",
                Obj::new()
                    .set("type", "default")
                    .opt("ttl", ttl_bucket(cache.ttl_seconds))
                    .build(),
            )
            .build(),
    )
}

fn text_with_cache(
    breakpoints: &mut Breakpoints,
    text: &str,
    cache: Option<&CacheHint>,
) -> Vec<Value> {
    let mut out = vec![Obj::new().set("text", text).build()];
    out.extend(cache_point(breakpoints, cache));
    out
}

fn lower_tool_choice(choice: &opencode_llm_api::ToolChoice) -> Result<Option<Value>, LlmError> {
    let empty = || Value::Object(Default::default());
    Ok(match match_tool_choice(ROUTE, choice)? {
        Choice::Auto => Some(Obj::new().set("auto", empty()).build()),
        Choice::None => None,
        Choice::Required => Some(Obj::new().set("any", empty()).build()),
        Choice::Tool(name) => Some(
            Obj::new()
                .set("tool", Obj::new().set("name", name).build())
                .build(),
        ),
    })
}

fn bytes_block(kind: &str, format: &str, name: Option<String>, bytes: String) -> Value {
    Obj::new()
        .set(
            kind,
            Obj::new()
                .set("format", format)
                .opt("name", name)
                .set("source", Obj::new().set("bytes", bytes).build())
                .build(),
        )
        .build()
}

/// `BedrockMedia.lower`: route by MIME into an image or document block.
fn lower_media(
    media_type: &str,
    data: &MediaData,
    filename: Option<&str>,
) -> Result<Value, LlmError> {
    let mime = media_type.to_ascii_lowercase();
    let lookup =
        |table: &[(&str, &'static str)]| table.iter().find(|(m, _)| *m == mime).map(|(_, f)| *f);
    let keys = |table: &[(&'static str, &str)]| table.iter().map(|(m, _)| *m).collect::<Vec<_>>();
    if let Some(format) = lookup(IMAGE_FORMATS) {
        let media = validate_media(ROUTE, media_type, data, &keys(IMAGE_FORMATS))?;
        return Ok(bytes_block("image", format, None, media.base64));
    }
    if mime.starts_with("image/") {
        return Err(LlmError::invalid_request(format!(
            "Bedrock Converse does not support image media type {media_type}"
        )));
    }
    if let Some(format) = lookup(DOCUMENT_FORMATS) {
        let media = validate_media(ROUTE, media_type, data, &keys(DOCUMENT_FORMATS))?;
        let name = filename.map_or_else(|| format!("document.{format}"), str::to_owned);
        return Ok(bytes_block("document", format, Some(name), media.base64));
    }
    Err(LlmError::invalid_request(format!(
        "Bedrock Converse does not support media type {media_type}"
    )))
}

fn lower_tool_result(part: &ToolResultPart) -> Result<Value, LlmError> {
    let content = match &part.result {
        ToolResultValue::Text(_) | ToolResultValue::Error(_) => {
            vec![Obj::new().set("text", tool_result_text(part)).build()]
        }
        ToolResultValue::Json(value) => vec![Obj::new().set("json", value.clone()).build()],
        ToolResultValue::Content(items) => items
            .iter()
            .map(|item| match item {
                ToolContent::Text { text } => Ok(Obj::new().set("text", text.clone()).build()),
                ToolContent::File { uri, mime, name } => {
                    let media = lower_media(mime, &MediaData::Text(uri.clone()), name.as_deref())?;
                    if media.get("image").is_none() {
                        return Err(LlmError::invalid_request(
                            "Bedrock Converse only supports image media in tool results",
                        ));
                    }
                    Ok(media)
                }
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(Obj::new()
        .set(
            "toolResult",
            Obj::new()
                .set("toolUseId", part.id.clone())
                .set("content", Value::Array(content))
                .set(
                    "status",
                    if matches!(part.result, ToolResultValue::Error(_)) {
                        "error"
                    } else {
                        "success"
                    },
                )
                .build(),
        )
        .build())
}

fn lower_messages(
    request: &LlmRequest,
    breakpoints: &mut Breakpoints,
) -> Result<Vec<Value>, LlmError> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                let texts = system_update_text(ROUTE, message)?;
                let cache = match message.content.last() {
                    Some(ContentPart::Text { cache, .. }) => cache.as_ref(),
                    _ => None,
                };
                let content =
                    text_with_cache(breakpoints, &wrap_system_update(&texts.join("\n")), cache);
                match out.last_mut() {
                    Some(("user", previous)) => previous.extend(content),
                    _ => out.push(("user", content)),
                }
            }
            MessageRole::User => {
                let mut content = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text, cache, .. } => {
                            content.extend(text_with_cache(breakpoints, text, cache.as_ref()))
                        }
                        ContentPart::Media {
                            media_type,
                            data,
                            filename,
                            ..
                        } => content.push(lower_media(media_type, data, filename.as_deref())?),
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
                            content.extend(text_with_cache(breakpoints, text, cache.as_ref()))
                        }
                        ContentPart::Reasoning {
                            text,
                            encrypted,
                            provider_metadata,
                            ..
                        } => content.push(
                            Obj::new()
                                .set(
                                    "reasoningContent",
                                    Obj::new()
                                        .set(
                                            "reasoningText",
                                            Obj::new()
                                                .set("text", text.clone())
                                                .opt(
                                                    "signature",
                                                    encrypted.clone().or_else(|| {
                                                        meta_str(
                                                            provider_metadata.as_ref(),
                                                            "bedrock",
                                                            "signature",
                                                        )
                                                    }),
                                                )
                                                .build(),
                                        )
                                        .build(),
                                )
                                .build(),
                        ),
                        ContentPart::ToolCall(call) => content.push(
                            Obj::new()
                                .set(
                                    "toolUse",
                                    Obj::new()
                                        .set("toolUseId", call.id.clone())
                                        .set("name", call.name.clone())
                                        .set("input", call.input.clone())
                                        .build(),
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
                out.push(("assistant", content));
            }
            MessageRole::Tool => {
                let mut content = Vec::new();
                for part in &message.content {
                    let ContentPart::ToolResult(result) = part else {
                        return Err(unsupported_content(ROUTE, "tool", &["tool-result"]));
                    };
                    content.push(lower_tool_result(result)?);
                    content.extend(cache_point(breakpoints, result.cache.as_ref()));
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
