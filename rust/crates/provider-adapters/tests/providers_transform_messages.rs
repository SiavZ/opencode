//! Purpose: exercise provider-specific message normalization, caching, unsupported media and option mapping.
//! Scope: offline typed-message examples; no live provider requests or full production parity claim.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/transform.rs
//! `-- provider-adapters/src/providers/transform_message.rs
//! External:
//! |-- indexmap::IndexMap::new
//! |-- serde_json::Map::new
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: assertions call the actual exported transforms and verify model-specific output.
//! Contract tests: this integration target runs fourteen message-transformation cases.

use opencode_llm_api::{
    ContentPart, Message, MessageRole, ToolCallPart, ToolResultPart, ToolResultValue,
};
use opencode_provider_adapters::providers::transform::TransformModel;
use opencode_provider_adapters::providers::transform_message::{
    apply_caching, map_provider_options, message as transform_messages, normalize_messages,
    unsupported_parts,
};
use serde_json::json;

fn make_model(npm: &str, provider_id: &str, api_id: &str) -> TransformModel {
    TransformModel {
        provider_id: provider_id.to_string(),
        api_id: api_id.to_string(),
        npm: npm.to_string(),
        url: String::new(),
        output_limit: 0,
        reasoning: false,
        input_modalities: vec![],
    }
}

#[test]
fn normalize_messages_sanitizes_text() {
    let msgs = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::text("Hello  world")],
    )];
    let model = make_model("@ai-sdk/openai", "openai", "gpt-4");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs[0].content.len(), 1);
    if let ContentPart::Text { text, .. } = &msgs[0].content[0] {
        assert_eq!(text, "Hello  world");
    } else {
        panic!("Expected Text part");
    }
}

#[test]
fn normalize_messages_filters_empty_for_anthropic() {
    let msgs = vec![
        Message::new(MessageRole::User, vec![ContentPart::text("")]),
        Message::new(MessageRole::Assistant, vec![ContentPart::text("Hello")]),
    ];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, MessageRole::Assistant);
}

#[test]
fn normalize_messages_filters_empty_reasoning_for_anthropic() {
    let msgs = vec![Message::new(
        MessageRole::Assistant,
        vec![ContentPart::reasoning("")],
    )];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs.len(), 0);
}

#[test]
fn normalize_messages_keeps_reasoning_with_signature() {
    let mut metadata = indexmap::IndexMap::new();
    let mut inner = serde_json::Map::new();
    inner.insert("signature".to_string(), json!("sig123"));
    metadata.insert("anthropic".to_string(), inner);

    let msgs = vec![Message::new(
        MessageRole::Assistant,
        vec![ContentPart::Reasoning {
            text: "".to_string(),
            encrypted: None,
            metadata: None,
            provider_metadata: Some(metadata),
        }],
    )];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs.len(), 1);
}

#[test]
fn normalize_messages_scrubs_claude_tool_ids() {
    let msgs = vec![Message::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall(ToolCallPart {
            id: "call-123!@#".to_string(),
            name: "test".to_string(),
            input: json!({}),
            provider_executed: None,
            metadata: None,
            provider_metadata: None,
        })],
    )];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    if let ContentPart::ToolCall(call) = &msgs[0].content[0] {
        assert_eq!(call.id, "call-123___");
    } else {
        panic!("Expected ToolCall");
    }
}

#[test]
fn normalize_messages_scrubs_mistral_tool_ids() {
    let msgs = vec![Message::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall(ToolCallPart {
            id: "call-123456789!@#".to_string(),
            name: "test".to_string(),
            input: json!({}),
            provider_executed: None,
            metadata: None,
            provider_metadata: None,
        })],
    )];
    let model = make_model("@ai-sdk/mistral", "mistral", "mistral-large");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    if let ContentPart::ToolCall(call) = &msgs[0].content[0] {
        assert_eq!(call.id, "call12345");
    } else {
        panic!("Expected ToolCall");
    }
}

#[test]
fn normalize_messages_adds_deepseek_reasoning() {
    let msgs = vec![Message::new(
        MessageRole::Assistant,
        vec![ContentPart::text("Hello")],
    )];
    let model = make_model("@ai-sdk/openai", "openai", "deepseek-chat");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs[0].content.len(), 2);
    assert!(matches!(msgs[0].content[1], ContentPart::Reasoning { .. }));
}

#[test]
fn normalize_messages_inserts_done_for_mistral() {
    let msgs = vec![
        Message::new(
            MessageRole::Tool,
            vec![ContentPart::ToolResult(ToolResultPart {
                id: "call12345".to_string(),
                name: "test".to_string(),
                result: ToolResultValue::Text(json!("result")),
                provider_executed: None,
                cache: None,
                metadata: None,
                provider_metadata: None,
            })],
        ),
        Message::new(MessageRole::User, vec![ContentPart::text("next")]),
    ];
    let model = make_model("@ai-sdk/mistral", "mistral", "mistral-large");
    let options = serde_json::Map::new();
    let msgs = normalize_messages(msgs, &model, &options);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1].role, MessageRole::Assistant);
    if let ContentPart::Text { text, .. } = &msgs[1].content[0] {
        assert_eq!(text, "Done.");
    } else {
        panic!("Expected Text part");
    }
}

#[test]
fn apply_caching_adds_metadata_to_anthropic() {
    let msgs = vec![
        Message::new(MessageRole::System, vec![ContentPart::text("sys1")]),
        Message::new(MessageRole::User, vec![ContentPart::text("user1")]),
        Message::new(MessageRole::Assistant, vec![ContentPart::text("asst1")]),
    ];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let msgs = apply_caching(msgs, &model);

    assert!(msgs[0].metadata.is_some());
    let meta = msgs[0].metadata.as_ref().unwrap();
    assert!(meta.contains_key("anthropic"));
    assert_eq!(
        meta["anthropic"],
        json!({ "cacheControl": { "type": "ephemeral" } })
    );

    assert!(msgs[1].metadata.is_some());
    assert!(msgs[2].metadata.is_some());
}

#[test]
fn apply_caching_adds_to_last_content_for_openrouter() {
    let msgs = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::text("user1")],
    )];
    let model = make_model("@openrouter/ai-sdk-provider", "openrouter", "gpt-4");
    let msgs = apply_caching(msgs, &model);

    if let ContentPart::Text {
        provider_metadata, ..
    } = &msgs[0].content[0]
    {
        assert!(provider_metadata.is_some());
        let meta = provider_metadata.as_ref().unwrap();
        assert!(meta.contains_key("openrouter"));
    } else {
        panic!("Expected Text part with metadata");
    }
}

#[test]
fn unsupported_parts_replaces_empty_base64() {
    let msgs = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::Media {
            media_type: "image/png".to_string(),
            data: opencode_llm_api::MediaData::Text("data:image/png;base64,".to_string()),
            filename: None,
            metadata: None,
        }],
    )];
    let model = make_model("@ai-sdk/openai", "openai", "gpt-4");
    let msgs = unsupported_parts(msgs, &model);

    if let ContentPart::Text { text, .. } = &msgs[0].content[0] {
        assert!(text.contains("empty or corrupted"));
    } else {
        panic!("Expected Text part");
    }
}

#[test]
fn map_provider_options_transforms_metadata() {
    let mut metadata = indexmap::IndexMap::new();
    metadata.insert("old_key".to_string(), serde_json::Map::new());

    let msgs = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::Text {
            text: "test".to_string(),
            cache: None,
            metadata: None,
            provider_metadata: Some(metadata),
        }],
    )];

    let msgs = map_provider_options(msgs, |mut meta| {
        if let Some(val) = meta.shift_remove("old_key") {
            meta.insert("new_key".to_string(), val);
        }
        Some(meta)
    });

    if let ContentPart::Text {
        provider_metadata, ..
    } = &msgs[0].content[0]
    {
        let meta = provider_metadata.as_ref().unwrap();
        assert!(!meta.contains_key("old_key"));
        assert!(meta.contains_key("new_key"));
    } else {
        panic!("Expected Text part");
    }
}

#[test]
fn transform_messages_orchestrates_all() {
    let msgs = vec![
        Message::new(MessageRole::System, vec![ContentPart::text("sys")]),
        Message::new(MessageRole::User, vec![ContentPart::text("user")]),
    ];
    let model = make_model("@ai-sdk/anthropic", "anthropic", "claude-3-sonnet");
    let options = serde_json::Map::new();
    let msgs = transform_messages(msgs, &model, &options);

    assert_eq!(msgs.len(), 2);
    assert!(msgs[0].metadata.is_some());
    assert!(msgs[1].metadata.is_some());
}

#[test]
fn transform_messages_remaps_provider_id_to_sdk_key() {
    let mut metadata = indexmap::IndexMap::new();
    let mut inner = serde_json::Map::new();
    inner.insert("key".to_string(), json!("value"));
    metadata.insert("custom-provider".to_string(), inner);

    let msgs = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::Text {
            text: "test".to_string(),
            cache: None,
            metadata: None,
            provider_metadata: Some(metadata),
        }],
    )];

    let model = make_model("@ai-sdk/openai", "custom-provider", "gpt-4");
    let options = serde_json::Map::new();
    let msgs = transform_messages(msgs, &model, &options);

    if let ContentPart::Text {
        provider_metadata, ..
    } = &msgs[0].content[0]
    {
        let meta = provider_metadata.as_ref().unwrap();
        assert!(!meta.contains_key("custom-provider"));
        assert!(meta.contains_key("openai"));
    } else {
        panic!("Expected Text part");
    }
}
