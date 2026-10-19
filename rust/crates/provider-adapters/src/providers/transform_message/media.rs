//! Purpose: replace empty or unsupported media parts with provider-compatible error text.
//! Scope: user-message content conversion against the selected model's input modalities.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/ids.rs
//! |-- llm-api/src/message.rs
//! `-- provider-adapters/src/providers/transform.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/transform_message.rs
//! Invariants: non-user messages and supported media remain untouched; replacements preserve the existing error text and clear media metadata.
//! Contract tests: provider-adapters/tests/providers_transform_messages.rs exercises the actual public re-export.

use crate::providers::transform::TransformModel;
use opencode_llm_api::ids::MessageRole;
use opencode_llm_api::message::{ContentPart, MediaData, Message};

fn mime_to_modality(mime: &str) -> Option<&'static str> {
    if mime.starts_with("image/") {
        return Some("image");
    }
    if mime.starts_with("audio/") {
        return Some("audio");
    }
    if mime.starts_with("video/") {
        return Some("video");
    }
    if mime == "application/pdf" {
        return Some("pdf");
    }
    None
}

pub fn unsupported_parts(mut msgs: Vec<Message>, model: &TransformModel) -> Vec<Message> {
    for msg in &mut msgs {
        if msg.role != MessageRole::User {
            continue;
        }
        for part in &mut msg.content {
            if let ContentPart::Media {
                media_type,
                data,
                filename,
                ..
            } = part
                && let MediaData::Text(data_str) = data
                && data_str.starts_with("data:")
                && let Some(idx) = data_str.find(',')
            {
                let base64_part = &data_str[idx + 1..];
                if base64_part.is_empty() {
                    *part = ContentPart::Text {
                        text:
                            "ERROR: Image file is empty or corrupted. Please provide a valid image."
                                .to_string(),
                        cache: None,
                        metadata: None,
                        provider_metadata: None,
                    };
                    continue;
                }
                let mime = if media_type.starts_with("data:") {
                    media_type
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .strip_prefix("data:")
                        .unwrap_or("")
                        .to_string()
                } else {
                    media_type.clone()
                };
                let modality = mime_to_modality(&mime);
                if let Some(modality_name) = modality {
                    let supported = model.input_modalities.iter().any(|m| m == modality_name);
                    if !supported {
                        let name = filename
                            .as_ref()
                            .map(|f| format!("\"{}\"", f))
                            .unwrap_or_else(|| modality_name.to_string());
                        *part = ContentPart::Text {
                            text: format!(
                                "ERROR: Cannot read {} (this model does not support {} input). Inform the user.",
                                name, modality_name
                            ),
                            cache: None,
                            metadata: None,
                            provider_metadata: None,
                        };
                    }
                }
            }
        }
    }
    msgs
}
