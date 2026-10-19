//! Purpose: message-side transformation rules ported from packages/opencode/src/provider/transform.ts
//! Scope: normalize_messages, apply_caching, unsupported_parts, map_provider_options, message
//! Uses:
//! rust/crates/
//! |-- llm-api/src/ids.rs
//! |-- llm-api/src/message.rs
//! |-- provider-adapters/src/providers/transform.rs
//! `-- provider-adapters/src/providers/transform_message/media.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/tests/providers_transform_messages.rs
//! Invariants: transformations preserve typed message structure while applying provider-specific rewrites; Rust strings already contain valid Unicode scalar values.
//! Contract tests: crates/provider-adapters/tests/providers_transform_messages.rs

mod media;

pub use media::unsupported_parts;

use opencode_llm_api::ids::{JsonObject, MessageRole, ProviderMetadata};
use opencode_llm_api::message::{ContentPart, Message, ToolContent, ToolResultValue};
use serde_json::{Value, json};

use crate::providers::transform::{TransformModel, sdk_key};

pub fn sanitize_surrogates(content: &str) -> String {
    content.to_string()
}

fn scrub_claude_tool_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn scrub_mistral_tool_id(id: &str) -> String {
    let mut cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(9)
        .collect();
    while cleaned.len() < 9 {
        cleaned.push('0');
    }
    cleaned
}

pub fn normalize_messages(
    mut msgs: Vec<Message>,
    model: &TransformModel,
    _options: &JsonObject,
) -> Vec<Message> {
    for msg in &mut msgs {
        for part in &mut msg.content {
            match part {
                ContentPart::Text { text, .. } | ContentPart::Reasoning { text, .. } => {
                    *text = sanitize_surrogates(text);
                }
                ContentPart::ToolResult(tr) => match &mut tr.result {
                    ToolResultValue::Text(Value::String(s))
                    | ToolResultValue::Error(Value::String(s)) => {
                        *s = sanitize_surrogates(s);
                    }
                    ToolResultValue::Content(parts) => {
                        for p in parts {
                            if let ToolContent::Text { text } = p {
                                *text = sanitize_surrogates(text);
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    if model.npm == "@ai-sdk/anthropic" {
        msgs = msgs
            .into_iter()
            .filter_map(|mut msg| {
                let mut filtered: Vec<ContentPart> = Vec::new();
                for part in msg.content {
                    match &part {
                        ContentPart::Text { text, .. } => {
                            if !text.is_empty() {
                                filtered.push(part);
                            }
                        }
                        ContentPart::Reasoning {
                            text,
                            provider_metadata,
                            ..
                        } => {
                            let has_sig = provider_metadata
                                .as_ref()
                                .and_then(|m| m.get("anthropic"))
                                .and_then(|v| v.get("signature"))
                                .is_some();
                            let has_redacted = provider_metadata
                                .as_ref()
                                .and_then(|m| m.get("anthropic"))
                                .and_then(|v| v.get("redactedData"))
                                .is_some();
                            if !text.trim().is_empty() || has_sig || has_redacted {
                                filtered.push(part);
                            }
                        }
                        _ => filtered.push(part),
                    }
                }
                if filtered.is_empty() {
                    None
                } else {
                    msg.content = filtered;
                    Some(msg)
                }
            })
            .collect();
    }

    if model.npm == "@ai-sdk/amazon-bedrock" {
        msgs = msgs
            .into_iter()
            .filter_map(|mut msg| {
                let mut filtered: Vec<ContentPart> = Vec::new();
                for part in msg.content {
                    match &part {
                        ContentPart::Text { text, .. } => {
                            if !text.is_empty() {
                                filtered.push(part);
                            }
                        }
                        ContentPart::Reasoning {
                            provider_metadata, ..
                        } => {
                            let meta = provider_metadata.as_ref();
                            let has_sig = meta
                                .and_then(|m| m.get(&model.provider_id))
                                .and_then(|v| v.get("signature"))
                                .is_some()
                                || meta
                                    .and_then(|m| m.get("bedrock"))
                                    .and_then(|v| v.get("signature"))
                                    .is_some();
                            let has_redacted = meta
                                .and_then(|m| m.get(&model.provider_id))
                                .and_then(|v| v.get("redactedContent"))
                                .is_some()
                                || meta
                                    .and_then(|m| m.get(&model.provider_id))
                                    .and_then(|v| v.get("redactedData"))
                                    .is_some()
                                || meta
                                    .and_then(|m| m.get("bedrock"))
                                    .and_then(|v| v.get("redactedContent"))
                                    .is_some()
                                || meta
                                    .and_then(|m| m.get("bedrock"))
                                    .and_then(|v| v.get("redactedData"))
                                    .is_some();
                            if has_sig || has_redacted {
                                filtered.push(part);
                            }
                        }
                        _ => filtered.push(part),
                    }
                }
                if filtered.is_empty() {
                    None
                } else {
                    msg.content = filtered;
                    Some(msg)
                }
            })
            .collect();
    }

    if model.api_id.to_lowercase().contains("claude") {
        for msg in &mut msgs {
            for part in &mut msg.content {
                match part {
                    ContentPart::ToolCall(tc) => tc.id = scrub_claude_tool_id(&tc.id),
                    ContentPart::ToolResult(tr) => tr.id = scrub_claude_tool_id(&tr.id),
                    _ => {}
                }
            }
        }
    }

    let model_id = model.api_id.to_lowercase();
    let is_mistral = model.provider_id == "mistral"
        || ["mistral", "devstral", "codestral", "pixtral", "mixtral"]
            .iter()
            .any(|f| model_id.contains(f));
    if is_mistral {
        let mut result: Vec<Message> = Vec::new();
        for i in 0..msgs.len() {
            let mut msg = msgs[i].clone();
            for part in &mut msg.content {
                match part {
                    ContentPart::ToolCall(tc) => tc.id = scrub_mistral_tool_id(&tc.id),
                    ContentPart::ToolResult(tr) => tr.id = scrub_mistral_tool_id(&tr.id),
                    _ => {}
                }
            }
            result.push(msg);
            if msgs[i].role == MessageRole::Tool
                && let Some(next) = msgs.get(i + 1)
                && next.role == MessageRole::User
            {
                result.push(Message {
                    id: None,
                    role: MessageRole::Assistant,
                    content: vec![ContentPart::Text {
                        text: "Done.".to_string(),
                        cache: None,
                        metadata: None,
                        provider_metadata: None,
                    }],
                    metadata: None,
                    native: None,
                });
            }
        }
        msgs = result;
    }

    if model.api_id.to_lowercase().contains("deepseek") {
        for msg in &mut msgs {
            if msg.role == MessageRole::Assistant {
                let has_reasoning = msg
                    .content
                    .iter()
                    .any(|p| matches!(p, ContentPart::Reasoning { .. }));
                if !has_reasoning {
                    msg.content.push(ContentPart::Reasoning {
                        text: String::new(),
                        encrypted: None,
                        metadata: None,
                        provider_metadata: None,
                    });
                }
            }
        }
    }

    msgs
}

fn cache_control_options() -> ProviderMetadata {
    let mut opts = ProviderMetadata::default();
    opts.insert(
        "anthropic".to_string(),
        json!({"cacheControl": {"type": "ephemeral"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts.insert(
        "openrouter".to_string(),
        json!({"cacheControl": {"type": "ephemeral"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts.insert(
        "bedrock".to_string(),
        json!({"cachePoint": {"type": "default"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts.insert(
        "openaiCompatible".to_string(),
        json!({"cache_control": {"type": "ephemeral"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts.insert(
        "copilot".to_string(),
        json!({"copilot_cache_control": {"type": "ephemeral"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts.insert(
        "alibaba".to_string(),
        json!({"cacheControl": {"type": "ephemeral"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    opts
}

pub fn apply_caching(mut msgs: Vec<Message>, model: &TransformModel) -> Vec<Message> {
    let system_indices: Vec<usize> = msgs
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == MessageRole::System)
        .take(2)
        .map(|(i, _)| i)
        .collect();
    let non_system_indices: Vec<usize> = msgs
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != MessageRole::System)
        .map(|(i, _)| i)
        .collect();
    let final_indices: Vec<usize> = non_system_indices.into_iter().rev().take(2).collect();

    let mut targets: Vec<usize> = system_indices;
    targets.extend(final_indices);
    targets.sort();
    targets.dedup();

    let cache_opts = cache_control_options();
    let use_message_level = model.provider_id == "anthropic"
        || model.provider_id.contains("bedrock")
        || model.npm == "@ai-sdk/amazon-bedrock";

    for ptr in targets {
        let msg = &mut msgs[ptr];
        let should_use_content = !use_message_level && !msg.content.is_empty();

        if should_use_content
            && let Some(last) = msg.content.last_mut()
            && let ContentPart::Text {
                provider_metadata, ..
            }
            | ContentPart::Reasoning {
                provider_metadata, ..
            } = last
        {
            let mut meta = provider_metadata
                .take()
                .unwrap_or_else(ProviderMetadata::default);
            for (k, v) in &cache_opts {
                meta.insert(k.clone(), v.clone());
            }
            *provider_metadata = Some(meta);
        } else if !should_use_content {
            let mut meta = msg.metadata.take().unwrap_or_default();
            for (k, v) in &cache_opts {
                meta.insert(k.clone(), Value::Object(v.clone()));
            }
            msg.metadata = Some(meta);
        }
    }
    msgs
}

pub fn map_provider_options<F>(mut msgs: Vec<Message>, mut transform: F) -> Vec<Message>
where
    F: FnMut(ProviderMetadata) -> Option<ProviderMetadata>,
{
    for msg in &mut msgs {
        if let Some(native) = msg.native.take() {
            let mut meta = ProviderMetadata::default();
            for (k, v) in native {
                if let Value::Object(obj) = v {
                    meta.insert(k, obj);
                }
            }
            if let Some(new_meta) = transform(meta) {
                let mut new_native = JsonObject::new();
                for (k, v) in new_meta {
                    new_native.insert(k, Value::Object(v));
                }
                msg.native = Some(new_native);
            }
        }
        for part in &mut msg.content {
            match part {
                ContentPart::Text {
                    provider_metadata, ..
                }
                | ContentPart::Reasoning {
                    provider_metadata, ..
                } => {
                    if let Some(meta) = provider_metadata.take()
                        && let Some(new_meta) = transform(meta)
                    {
                        *provider_metadata = Some(new_meta);
                    }
                }
                ContentPart::ToolCall(tc) => {
                    if let Some(meta) = tc.provider_metadata.take()
                        && let Some(new_meta) = transform(meta)
                    {
                        tc.provider_metadata = Some(new_meta);
                    }
                }
                ContentPart::ToolResult(tr) => {
                    if let Some(meta) = tr.provider_metadata.take()
                        && let Some(new_meta) = transform(meta)
                    {
                        tr.provider_metadata = Some(new_meta);
                    }
                }
                _ => {}
            }
        }
    }
    msgs
}

pub fn message(
    mut msgs: Vec<Message>,
    model: &TransformModel,
    options: &JsonObject,
) -> Vec<Message> {
    msgs = unsupported_parts(msgs, model);
    msgs = normalize_messages(msgs, model, options);

    let uses_anthropic_automatic_caching = options.get("cacheControl").is_some()
        && (model.npm == "@ai-sdk/anthropic" || model.npm == "@ai-sdk/google-vertex/anthropic");

    let is_anthropic_like = model.provider_id == "anthropic"
        || model.provider_id == "google-vertex-anthropic"
        || model.api_id.to_lowercase().contains("anthropic")
        || model.api_id.to_lowercase().contains("claude")
        || model.npm == "@ai-sdk/anthropic"
        || model.npm == "@ai-sdk/alibaba";

    if is_anthropic_like && model.npm != "@ai-sdk/gateway" && !uses_anthropic_automatic_caching {
        msgs = apply_caching(msgs, model);
    }

    if let Some(key) = sdk_key(&model.npm)
        && key != model.provider_id
    {
        let provider_id = model.provider_id.clone();
        let key_str = key.to_string();
        msgs = map_provider_options(msgs, |opts| {
            let mut opts = opts;
            if !opts.contains_key(&provider_id) {
                return Some(opts);
            }
            if let Some(val) = opts.swap_remove(&provider_id) {
                opts.insert(key_str.clone(), val);
            }
            Some(opts)
        });
    }

    let openai_npm = [
        "@ai-sdk/openai",
        "@ai-sdk/azure",
        "@ai-sdk/amazon-bedrock/mantle",
        "@ai-sdk/github-copilot",
    ];
    if options.get("store") != Some(&Value::Bool(true))
        && sdk_key(&model.npm).is_some()
        && openai_npm.contains(&model.npm.as_str())
    {
        let key_str = sdk_key(&model.npm).unwrap().to_string();
        msgs = map_provider_options(msgs, |opts| {
            let mut opts = opts;
            if let Some(mut meta) = opts.swap_remove(&key_str) {
                meta.remove("itemId");
                opts.insert(key_str.clone(), meta);
            }
            Some(opts)
        });
    }

    msgs
}
