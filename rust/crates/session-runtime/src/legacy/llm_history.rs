//! Purpose: convert persisted legacy messages into provider-neutral messages without V2 translation.
//! Scope: V1 user text/media, assistant text/reasoning/tool calls and tool settlements.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/legacy/llm_loop.rs
//! Invariants: ignored text is excluded and tool results follow their assistant call message.
//! Contract tests: runtime/tests/legacy_model.rs

use opencode_llm_api::{
    ContentPart, MediaData, Message, MessageRole, ToolCallPart, ToolResultPart, ToolResultValue,
};
use serde_json::{Value, json};

use super::types::{LegacyError, WithParts, part_type, str_field};

pub(super) fn messages(history: &[WithParts]) -> Result<Vec<Message>, LegacyError> {
    let mut messages = Vec::new();
    let completed = super::compaction::completed_compactions(history);
    let latest = completed.last();
    if let Some(summary) = latest.and_then(|latest| latest.summary.as_deref()) {
        messages.push(Message::user(format!("Conversation summary:\n{summary}")));
    }
    let tail = latest
        .and_then(|latest| {
            history[latest.user_index]
                .parts
                .iter()
                .find_map(|part| str_field(part, "tail_start_id"))
        })
        .and_then(|id| history.iter().position(|row| row.id() == id));
    for (index, row) in history.iter().enumerate().filter(|(index, _)| {
        latest.is_none_or(|latest| {
            *index > latest.assistant_index
                || tail.is_some_and(|tail| *index >= tail && *index < latest.user_index)
        })
    }) {
        let _ = index;
        let mut content = Vec::new();
        let mut results = Vec::new();
        for part in &row.parts {
            match part_type(part) {
                "text" if part.get("ignored") != Some(&Value::Bool(true)) => {
                    if let Some(text) = str_field(part, "text").filter(|text| !text.is_empty()) {
                        content.push(ContentPart::text(text));
                    }
                }
                "reasoning" if row.is_assistant() => {
                    if let Some(text) = str_field(part, "text").filter(|text| !text.is_empty()) {
                        content.push(ContentPart::reasoning(text));
                    }
                }
                // The resolver already emits readable text for these files. Sending the
                // original file as media makes OpenAI reject text/plain attachments.
                "file"
                    if row.is_user()
                        && !matches!(
                            str_field(part, "mime"),
                            Some("text/plain" | "application/x-directory")
                        ) =>
                {
                    content.push(ContentPart::Media {
                        media_type: str_field(part, "mime").unwrap_or_default().into(),
                        data: MediaData::Text(str_field(part, "url").unwrap_or_default().into()),
                        filename: str_field(part, "filename").map(str::to_owned),
                        metadata: None,
                    });
                }
                "tool" if row.is_assistant() => {
                    let id = str_field(part, "callID").unwrap_or_default().to_owned();
                    let name = str_field(part, "tool").unwrap_or_default().to_owned();
                    let input = part
                        .get("state")
                        .and_then(|state| state.get("input"))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let provider = part
                        .get("metadata")
                        .and_then(|metadata| metadata.get("providerExecuted"))
                        .and_then(Value::as_bool);
                    content.push(ContentPart::ToolCall(ToolCallPart {
                        id: id.clone(),
                        name: name.clone(),
                        input,
                        provider_executed: provider,
                        metadata: None,
                        provider_metadata: None,
                    }));
                    let state = &part["state"];
                    let result = match state["status"].as_str() {
                        Some("completed") => ToolResultValue::Text(
                            if state["time"]
                                .get("compacted")
                                .is_some_and(|value| !value.is_null())
                            {
                                json!("[Old tool result content cleared]")
                            } else {
                                state.get("output").cloned().unwrap_or_else(|| json!(""))
                            },
                        ),
                        Some("error") => ToolResultValue::Error(
                            state
                                .get("error")
                                .cloned()
                                .unwrap_or_else(|| json!("Tool failed")),
                        ),
                        _ => ToolResultValue::Error(json!("Tool execution aborted")),
                    };
                    results.push(Message::tool(ToolResultPart {
                        id,
                        name,
                        result,
                        provider_executed: provider,
                        cache: None,
                        metadata: None,
                        provider_metadata: None,
                    }));
                }
                _ => {}
            }
        }
        if !content.is_empty() {
            messages.push(Message::new(
                if row.is_user() {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content,
            ));
            messages.extend(results);
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_attachment_stays_text_for_prompt_and_title_history() {
        let history = vec![WithParts {
            info: json!({"id":"user-1","role":"user"}).as_object().unwrap().clone(),
            parts: [
                json!({"type":"file","mime":"text/plain","filename":"stdin.txt","url":"data:text/plain;base64,cGlwZWQ="}),
                json!({"type":"text","text":"piped input"}),
                json!({"type":"file","mime":"application/x-directory","url":"file:///repo/"}),
                json!({"type":"file","mime":"image/png","filename":"image.png","url":"data:image/png;base64,aGVsbG8="}),
            ]
            .into_iter()
            .map(|part| part.as_object().unwrap().clone())
            .collect(),
        }];

        let converted = messages(&history).unwrap();
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].content.len(), 2);
        assert!(
            matches!(&converted[0].content[0], ContentPart::Text { text, .. } if text == "piped input")
        );
        assert!(
            matches!(&converted[0].content[1], ContentPart::Media { media_type, .. } if media_type == "image/png")
        );
    }
}
