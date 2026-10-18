//! Purpose: helpers shared by every protocol state machine: block lifecycle (step-start,
//! text/reasoning open/close, finish), streamed tool-call accumulation, tool-input parsing,
//! token arithmetic, media validation and text joins (protocols/shared.ts, utils/lifecycle.ts,
//! utils/tool-stream.ts).
//! Scope: pure helpers.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! |-- base64::engine::general_purpose::STANDARD
//! `-- indexmap::{IndexMap, IndexSet}
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: lifecycle `finish` closes reasoning blocks before text blocks, then emits
//! `step-finish` (index 0) and `finish`. Empty streamed tool input parses as `{}`.
//! Contract tests: provider-adapters/src/shared.rs (unit tests)

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use indexmap::{IndexMap, IndexSet};
use opencode_llm_api::{
    ContentPart, FinishReason, LlmError, LlmEvent, MediaData, Message, ProviderMetadata,
    ToolResultPart, ToolResultValue, Usage,
};
use serde_json::Value;

pub use opencode_llm_api::event::{subtract_tokens, total_tokens};

/// `sumTokens`: `None` only when every value is `None`.
pub fn sum_tokens(values: &[Option<f64>]) -> Option<f64> {
    if values.iter().all(Option::is_none) {
        return None;
    }
    Some(values.iter().map(|v| v.unwrap_or(0.0)).sum())
}

/// `parseToolInput`: empty input means `{}`.
pub fn parse_tool_input(route: &str, name: &str, raw: &str) -> Result<Value, LlmError> {
    let text = if raw.is_empty() { "{}" } else { raw };
    serde_json::from_str(text).map_err(|_| {
        LlmError::invalid_output(
            route,
            format!("Invalid JSON input for {route} tool call {name}"),
            Some(text.into()),
        )
    })
}

/// `joinText`.
pub fn join_text<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().collect::<Vec<_>>().join("\n")
}

/// `wrapSystemUpdate`: lower a chronological system message into escaped user text.
pub fn wrap_system_update(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<system-update>\n{escaped}\n</system-update>")
}

/// `systemUpdateText`: chronological system messages accept text only.
pub fn system_update_text(route: &str, message: &Message) -> Result<Vec<String>, LlmError> {
    message
        .content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text, .. } => Ok(text.clone()),
            _ => Err(unsupported_content(route, "system", &["text"])),
        })
        .collect()
}

/// `unsupportedContent`: `<route> <role> messages only support a, b, and c content for now`.
pub fn unsupported_content(route: &str, role: &str, types: &[&str]) -> LlmError {
    let list = match types {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    LlmError::invalid_request(format!(
        "{route} {role} messages only support {list} content for now"
    ))
}

/// `toolResultText`.
pub fn tool_result_text(part: &ToolResultPart) -> String {
    match &part.result {
        ToolResultValue::Text(Value::String(s)) => s.clone(),
        ToolResultValue::Text(v) => js_string(v),
        ToolResultValue::Error(v @ (Value::Object(_) | Value::Array(_))) => v.to_string(),
        ToolResultValue::Error(v) => js_string(v),
        ToolResultValue::Json(v) => v.to_string(),
        ToolResultValue::Content(c) => serde_json::to_string(c).unwrap_or_default(),
    }
}

/// JS `String(value)` for JSON values.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

pub const IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
pub const VIDEO_MIMES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];
pub const AUDIO_MIMES: &[&str] = &[
    "audio/wav",
    "audio/mp3",
    "audio/aiff",
    "audio/aac",
    "audio/ogg",
    "audio/flac",
];
pub const MAX_MEDIA_ENCODED_BYTES: usize = 28 * 1024 * 1024;
pub const MAX_MEDIA_DECODED_BYTES: usize = 20 * 1024 * 1024;

/// Validated media payload.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedMedia {
    pub mime: String,
    pub base64: String,
    pub data_url: String,
    pub bytes: Vec<u8>,
}

/// `validateMedia`.
pub fn validate_media(
    route: &str,
    media_type: &str,
    data: &MediaData,
    supported: &[&str],
) -> Result<ValidatedMedia, LlmError> {
    let invalid = |m: String| LlmError::invalid_request(m);
    let mime = media_type.to_ascii_lowercase();
    if !supported.contains(&mime.as_str()) {
        return Err(invalid(format!(
            "{route} does not support media type {media_type}"
        )));
    }
    let base64 = match data {
        MediaData::Bytes(bytes) => {
            if bytes.len() > MAX_MEDIA_DECODED_BYTES {
                return Err(invalid(format!(
                    "{route} media exceeds the {MAX_MEDIA_DECODED_BYTES} byte decoded limit"
                )));
            }
            STANDARD.encode(bytes)
        }
        MediaData::Text(text) if text.starts_with("data:") => {
            let rest = &text[5..];
            let (header, payload) = rest.split_once(',').ok_or_else(|| {
                invalid(format!("{route} media data URL must contain valid base64"))
            })?;
            let declared = header
                .strip_suffix(";base64")
                .filter(|h| !h.is_empty() && !h.contains(';'));
            let Some(declared) = declared else {
                return Err(invalid(format!(
                    "{route} media data URL must contain valid base64"
                )));
            };
            if !payload
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
            {
                return Err(invalid(format!(
                    "{route} media data URL must contain valid base64"
                )));
            }
            if declared.to_ascii_lowercase() != mime {
                return Err(invalid(format!(
                    "{route} media type {media_type} does not match data URL type {declared}"
                )));
            }
            payload.to_owned()
        }
        MediaData::Text(text) => text.clone(),
    };
    if base64.len() > MAX_MEDIA_ENCODED_BYTES {
        return Err(invalid(format!(
            "{route} media exceeds the {MAX_MEDIA_ENCODED_BYTES} byte encoded limit"
        )));
    }
    let bytes = match STANDARD.decode(&base64) {
        Ok(bytes) if !base64.is_empty() && base64.len() % 4 == 0 => bytes,
        _ => return Err(invalid(format!("{route} media must contain valid base64"))),
    };
    if bytes.len() > MAX_MEDIA_DECODED_BYTES {
        return Err(invalid(format!(
            "{route} media exceeds the {MAX_MEDIA_DECODED_BYTES} byte decoded limit"
        )));
    }
    if STANDARD.encode(&bytes) != base64 {
        return Err(invalid(format!(
            "{route} media must contain canonical base64"
        )));
    }
    let data_url = format!("data:{mime};base64,{base64}");
    Ok(ValidatedMedia {
        mime,
        base64,
        data_url,
        bytes,
    })
}

/// Block lifecycle state (`utils/lifecycle.ts`).
#[derive(Clone, Debug, Default)]
pub struct Lifecycle {
    pub step_started: bool,
    pub text: IndexSet<String>,
    pub reasoning: IndexSet<String>,
}

impl Lifecycle {
    pub fn step_start(&mut self, events: &mut Vec<LlmEvent>) {
        if !self.step_started {
            events.push(LlmEvent::StepStart { index: 0 });
            self.step_started = true;
        }
    }

    pub fn text_delta(&mut self, events: &mut Vec<LlmEvent>, id: &str, text: &str) {
        self.step_start(events);
        if self.text.insert(id.to_owned()) {
            events.push(LlmEvent::TextStart {
                id: id.into(),
                provider_metadata: None,
            });
        }
        events.push(LlmEvent::TextDelta {
            id: id.into(),
            text: text.into(),
            provider_metadata: None,
        });
    }

    pub fn reasoning_start(
        &mut self,
        events: &mut Vec<LlmEvent>,
        id: &str,
        provider_metadata: Option<ProviderMetadata>,
    ) {
        if self.reasoning.contains(id) {
            return;
        }
        self.step_start(events);
        events.push(LlmEvent::ReasoningStart {
            id: id.into(),
            provider_metadata,
        });
        self.reasoning.insert(id.to_owned());
    }

    pub fn reasoning_delta(
        &mut self,
        events: &mut Vec<LlmEvent>,
        id: &str,
        text: &str,
        provider_metadata: Option<ProviderMetadata>,
    ) {
        self.reasoning_start(events, id, provider_metadata);
        events.push(LlmEvent::ReasoningDelta {
            id: id.into(),
            text: text.into(),
            provider_metadata: None,
        });
    }

    pub fn reasoning_end(
        &mut self,
        events: &mut Vec<LlmEvent>,
        id: &str,
        provider_metadata: Option<ProviderMetadata>,
    ) {
        if !self.reasoning.contains(id) {
            return;
        }
        self.step_start(events);
        events.push(LlmEvent::ReasoningEnd {
            id: id.into(),
            provider_metadata,
        });
        self.reasoning.shift_remove(id);
    }

    pub fn text_end(
        &mut self,
        events: &mut Vec<LlmEvent>,
        id: &str,
        provider_metadata: Option<ProviderMetadata>,
    ) {
        if !self.text.contains(id) {
            return;
        }
        self.step_start(events);
        events.push(LlmEvent::TextEnd {
            id: id.into(),
            provider_metadata,
        });
        self.text.shift_remove(id);
    }

    /// Close open blocks, then `step-finish` + `finish`.
    pub fn finish(
        &mut self,
        events: &mut Vec<LlmEvent>,
        reason: FinishReason,
        usage: Option<Usage>,
        provider_metadata: Option<ProviderMetadata>,
    ) {
        self.step_start(events);
        for id in self.reasoning.drain(..) {
            events.push(LlmEvent::ReasoningEnd {
                id,
                provider_metadata: None,
            });
        }
        for id in self.text.drain(..) {
            events.push(LlmEvent::TextEnd {
                id,
                provider_metadata: None,
            });
        }
        events.push(LlmEvent::StepFinish {
            index: 0,
            reason,
            usage: usage.clone(),
            provider_metadata: provider_metadata.clone(),
        });
        events.push(LlmEvent::Finish {
            reason,
            usage,
            provider_metadata,
        });
        self.step_started = false;
    }
}

/// One streamed tool call under assembly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PendingTool {
    pub id: String,
    pub name: String,
    pub input: String,
    pub provider_executed: Option<bool>,
    pub provider_metadata: Option<ProviderMetadata>,
}

/// `ToolStream` state keyed by provider index or item id, in first-seen order.
#[derive(Clone, Debug, Default)]
pub struct ToolStream<K: std::hash::Hash + Eq + Clone> {
    pub tools: IndexMap<K, PendingTool>,
}

impl<K: std::hash::Hash + Eq + Clone> ToolStream<K> {
    /// `start`: register without emitting.
    pub fn start(&mut self, key: K, tool: PendingTool) {
        self.tools.insert(key, tool);
    }

    /// `appendOrStart`: emits `tool-input-start` once, then deltas for non-empty text.
    pub fn append_or_start(
        &mut self,
        route: &str,
        key: K,
        id: Option<&str>,
        name: Option<&str>,
        text: &str,
        missing: &str,
    ) -> Result<Vec<LlmEvent>, LlmError> {
        let current = self.tools.get(&key).cloned();
        let id = id
            .map(str::to_owned)
            .or_else(|| current.as_ref().map(|c| c.id.clone()));
        let name = name
            .map(str::to_owned)
            .or_else(|| current.as_ref().map(|c| c.name.clone()));
        let (Some(id), Some(name)) = (id.filter(|s| !s.is_empty()), name.filter(|s| !s.is_empty()))
        else {
            return Err(LlmError::invalid_output(route, missing, None));
        };
        if let Some(c) = &current
            && text.is_empty()
            && c.id == id
            && c.name == name
        {
            return Ok(vec![]);
        }
        let tool = PendingTool {
            input: format!(
                "{}{}",
                current.as_ref().map(|c| c.input.as_str()).unwrap_or(""),
                text
            ),
            id,
            name,
            provider_executed: current.as_ref().and_then(|c| c.provider_executed),
            provider_metadata: current.as_ref().and_then(|c| c.provider_metadata.clone()),
        };
        Ok(self.append(key, tool, text, current.is_none()))
    }

    /// `appendExisting`.
    pub fn append_existing(
        &mut self,
        route: &str,
        key: K,
        text: &str,
        missing: &str,
    ) -> Result<Vec<LlmEvent>, LlmError> {
        let Some(current) = self.tools.get(&key).cloned() else {
            return Err(LlmError::invalid_output(route, missing, None));
        };
        if text.is_empty() {
            return Ok(vec![]);
        }
        let tool = PendingTool {
            input: format!("{}{text}", current.input),
            ..current
        };
        Ok(self.append(key, tool, text, false))
    }

    fn append(&mut self, key: K, tool: PendingTool, text: &str, is_new: bool) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        if is_new {
            events.push(LlmEvent::ToolInputStart {
                id: tool.id.clone(),
                name: tool.name.clone(),
                provider_metadata: tool.provider_metadata.clone(),
            });
        }
        if !text.is_empty() {
            events.push(LlmEvent::ToolInputDelta {
                id: tool.id.clone(),
                name: tool.name.clone(),
                text: text.into(),
            });
        }
        self.tools.insert(key, tool);
        events
    }

    /// `finish` / `finishWithInput`: `tool-input-end` + parsed `tool-call`.
    pub fn finish(
        &mut self,
        route: &str,
        key: &K,
        input: Option<&str>,
    ) -> Result<Vec<LlmEvent>, LlmError> {
        let Some(tool) = self.tools.shift_remove(key) else {
            return Ok(vec![]);
        };
        Self::complete(route, tool, input)
    }

    /// `finishAll` in first-seen order.
    pub fn finish_all(&mut self, route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        let mut events = Vec::new();
        for (_, tool) in self.tools.drain(..).collect::<Vec<_>>() {
            events.extend(Self::complete(route, tool, None)?);
        }
        Ok(events)
    }

    fn complete(
        route: &str,
        tool: PendingTool,
        input: Option<&str>,
    ) -> Result<Vec<LlmEvent>, LlmError> {
        let parsed = parse_tool_input(route, &tool.name, input.unwrap_or(&tool.input))?;
        Ok(vec![
            LlmEvent::ToolInputEnd {
                id: tool.id.clone(),
                name: tool.name.clone(),
                provider_metadata: tool.provider_metadata.clone(),
            },
            LlmEvent::ToolCall {
                id: tool.id,
                name: tool.name,
                input: parsed,
                provider_executed: tool.provider_executed.filter(|v| *v),
                provider_metadata: tool.provider_metadata,
            },
        ])
    }
}

/// Wrap a raw provider usage/metadata payload as `{ provider: payload }`.
pub fn provider_metadata(provider: &str, payload: Value) -> Option<ProviderMetadata> {
    match payload {
        Value::Object(map) => Some(IndexMap::from([(provider.to_owned(), map)])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_finish_closes_reasoning_then_text() {
        let mut lc = Lifecycle::default();
        let mut ev = Vec::new();
        lc.text_delta(&mut ev, "t", "hi");
        lc.reasoning_delta(&mut ev, "r", "think", None);
        lc.finish(&mut ev, FinishReason::Stop, None, None);
        let kinds: Vec<&str> = ev.iter().map(LlmEvent::kind).collect();
        assert_eq!(
            kinds,
            [
                "step-start",
                "text-start",
                "text-delta",
                "reasoning-start",
                "reasoning-delta",
                "reasoning-end",
                "text-end",
                "step-finish",
                "finish"
            ]
        );
    }

    #[test]
    fn tool_stream_accumulates_and_parses() {
        let mut ts = ToolStream::<u32>::default();
        let a = ts
            .append_or_start("r", 0, Some("c1"), Some("f"), "{\"a\"", "missing")
            .unwrap();
        assert_eq!(a.len(), 2);
        let b = ts
            .append_or_start("r", 0, None, None, ":1}", "missing")
            .unwrap();
        assert_eq!(b.len(), 1);
        let done = ts.finish_all("r").unwrap();
        assert_eq!(
            done[1],
            LlmEvent::ToolCall {
                id: "c1".into(),
                name: "f".into(),
                input: serde_json::json!({"a":1}),
                provider_executed: None,
                provider_metadata: None
            }
        );
        let err = ts
            .append_or_start("r", 1, None, None, "x", "Missing tool")
            .unwrap_err();
        assert_eq!(err.reason.message(), "Missing tool");
        assert_eq!(
            parse_tool_input("r", "f", "").unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn media_validation_matches_baseline_messages() {
        let ok = validate_media(
            "r",
            "image/PNG",
            &MediaData::Text("data:image/png;base64,aGk=".into()),
            IMAGE_MIMES,
        )
        .unwrap();
        assert_eq!(ok.data_url, "data:image/png;base64,aGk=");
        let err = validate_media(
            "r",
            "image/png",
            &MediaData::Text("data:image/gif;base64,aGk=".into()),
            IMAGE_MIMES,
        )
        .unwrap_err();
        assert_eq!(
            err.reason.message(),
            "r media type image/png does not match data URL type image/gif"
        );
        let err = validate_media(
            "r",
            "image/png",
            &MediaData::Text("aGl".into()),
            IMAGE_MIMES,
        )
        .unwrap_err();
        assert_eq!(err.reason.message(), "r media must contain valid base64");
        assert_eq!(
            unsupported_content("r", "user", &["text", "media", "tool-call"])
                .reason
                .message(),
            "r user messages only support text, media, and tool-call content for now"
        );
        assert_eq!(
            wrap_system_update("<a&b>"),
            "<system-update>\n&lt;a&amp;b&gt;\n</system-update>"
        );
    }
}
