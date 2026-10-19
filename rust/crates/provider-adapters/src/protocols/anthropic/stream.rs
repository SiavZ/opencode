//! Purpose: Anthropic Messages SSE stream state machine (anthropic-messages.ts `step`):
//! message_start/message_delta usage merge, text/thinking/signature deltas, streamed tool input,
//! server tool use and server tool results as provider-executed events, stop reasons and
//! mid-stream `error` events as `provider-error`.
//! Scope: pure parser over decoded frames.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::de::Error::custom
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::from_value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/anthropic/mod.rs
//! Invariants: usage merge is right-biased per field and recomputes the inclusive input total;
//! no `on_halt` flush (a stream without `message_delta` has no terminal finish, as baseline).
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

use opencode_llm_api::{
    FinishReason, LlmError, LlmEvent, ProviderFailureClassification, ToolResultValue, Usage,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::wire::{decode_event, meta};
use crate::protocol::{Frame, ProtocolState};
use crate::shared::{Lifecycle, PendingTool, ToolStream, js_string, sum_tokens, total_tokens};
use crate::transport::is_context_overflow;

const ADAPTER: &str = "anthropic-messages";
const USAGE_KEYS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
];

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: Option<f64>,
    #[serde(default)]
    output_tokens: Option<f64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<f64>,
    #[serde(default)]
    cache_read_input_tokens: Option<f64>,
}

/// Usage plus the schema-projected raw payload for `providerMetadata.anthropic`.
#[derive(Clone, Debug)]
struct AnthropicUsage {
    input_tokens: Option<f64>,
    output_tokens: Option<f64>,
    cache_write: Option<f64>,
    cache_read: Option<f64>,
    raw: Map<String, Value>,
}

impl<'de> Deserialize<'de> for AnthropicUsage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Map::<String, Value>::deserialize(d)?;
        let typed: RawUsage = serde_json::from_value(Value::Object(value.clone()))
            .map_err(serde::de::Error::custom)?;
        let raw = USAGE_KEYS
            .iter()
            .filter_map(|k| value.get(*k).map(|v| ((*k).to_owned(), v.clone())))
            .collect();
        Ok(Self {
            input_tokens: typed.input_tokens,
            output_tokens: typed.output_tokens,
            cache_write: typed.cache_creation_input_tokens,
            cache_read: typed.cache_read_input_tokens,
            raw,
        })
    }
}

#[derive(Deserialize, Default)]
struct StartMessage {
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    content: Option<Value>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
}

#[derive(Deserialize, Default)]
struct ErrorBody {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    index: Option<f64>,
    #[serde(default)]
    message: Option<StartMessage>,
    #[serde(default)]
    content_block: Option<Block>,
    #[serde(default)]
    delta: Option<Delta>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    error: Option<ErrorBody>,
}

/// Parser state for one Anthropic response.
#[derive(Default)]
pub(crate) struct State {
    tools: ToolStream<i64>,
    usage: Option<(Usage, Map<String, Value>)>,
    lifecycle: Lifecycle,
}

fn map_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("end_turn" | "stop_sequence" | "pause_turn") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        Some("refusal") => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

fn build_usage(
    non_cached: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    output: Option<f64>,
    raw: Map<String, Value>,
) -> (Usage, Map<String, Value>) {
    let input = sum_tokens(&[non_cached, cache_read, cache_write]);
    let usage = Usage {
        input_tokens: input,
        output_tokens: output,
        non_cached_input_tokens: non_cached,
        cache_read_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
        reasoning_tokens: None,
        total_tokens: total_tokens(input, output, None),
        provider_metadata: Some(
            [("anthropic".to_owned(), raw.clone())]
                .into_iter()
                .collect(),
        ),
    };
    (usage, raw)
}

/// `mapUsage`: Anthropic `input_tokens` is the non-cached count.
fn map_usage(usage: AnthropicUsage) -> (Usage, Map<String, Value>) {
    build_usage(
        usage.input_tokens,
        usage.cache_read,
        usage.cache_write,
        usage.output_tokens,
        usage.raw,
    )
}

/// `mergeUsage`: right-biased per field, inclusive input recomputed.
fn merge_usage(
    left: Option<(Usage, Map<String, Value>)>,
    right: Option<(Usage, Map<String, Value>)>,
) -> Option<(Usage, Map<String, Value>)> {
    let Some((l, lraw)) = left else { return right };
    let Some((r, rraw)) = right else {
        return Some((l, lraw));
    };
    let mut raw = lraw;
    for (k, v) in rraw {
        raw.insert(k, v);
    }
    Some(build_usage(
        r.non_cached_input_tokens.or(l.non_cached_input_tokens),
        r.cache_read_input_tokens.or(l.cache_read_input_tokens),
        r.cache_write_input_tokens.or(l.cache_write_input_tokens),
        r.output_tokens.or(l.output_tokens),
        raw,
    ))
}

fn server_tool_result_name(kind: &str) -> Option<&'static str> {
    match kind {
        "web_search_tool_result" => Some("web_search"),
        "code_execution_tool_result" => Some("code_execution"),
        "web_fetch_tool_result" => Some("web_fetch"),
        _ => None,
    }
}

fn server_tool_result(block: &Block) -> Option<LlmEvent> {
    let name = server_tool_result_name(&block.kind)?;
    let content = block.content.clone().unwrap_or(Value::Null);
    let error_type = match &content {
        Value::Object(map) => map.get("type").map(js_string).unwrap_or_default(),
        _ => String::new(),
    };
    let result = if error_type.ends_with("_tool_result_error") {
        ToolResultValue::Error(content)
    } else {
        ToolResultValue::Json(content)
    };
    Some(LlmEvent::ToolResult {
        id: block.tool_use_id.clone().unwrap_or_default(),
        name: name.into(),
        result,
        output: None,
        provider_executed: Some(true),
        provider_metadata: Some(meta("anthropic", "blockType", block.kind.clone())),
    })
}

/// `providerErrorMessage`: prefix the error type when both are present.
fn provider_error_message(error: Option<&ErrorBody>) -> String {
    let kind = error
        .and_then(|e| e.kind.as_deref())
        .filter(|s| !s.is_empty());
    let message = error
        .and_then(|e| e.message.as_deref())
        .filter(|s| !s.is_empty());
    match (kind, message) {
        (Some(k), Some(m)) => format!("{k}: {m}"),
        (_, Some(m)) => m.to_owned(),
        (Some(k), None) => k.to_owned(),
        (None, None) => "Anthropic Messages stream error".into(),
    }
}

impl State {
    fn content_block_start(&mut self, index: Option<i64>, block: Block) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        if (block.kind == "tool_use" || block.kind == "server_tool_use")
            && let Some(index) = index
        {
            self.lifecycle.step_start(&mut events);
            let id = block.id.clone().unwrap_or_else(|| index.to_string());
            let name = block.name.clone().unwrap_or_default();
            self.tools.start(
                index,
                PendingTool {
                    id: id.clone(),
                    name: name.clone(),
                    input: String::new(),
                    provider_executed: Some(block.kind == "server_tool_use"),
                    provider_metadata: None,
                },
            );
            events.push(LlmEvent::ToolInputStart {
                id,
                name,
                provider_metadata: None,
            });
            return events;
        }
        let at = index.unwrap_or(0);
        if block.kind == "text"
            && let Some(text) = block.text.as_deref().filter(|t| !t.is_empty())
        {
            self.lifecycle
                .text_delta(&mut events, &format!("text-{at}"), text);
            return events;
        }
        if block.kind == "thinking"
            && let Some(text) = block.thinking.as_deref().filter(|t| !t.is_empty())
        {
            self.lifecycle
                .reasoning_delta(&mut events, &format!("reasoning-{at}"), text, None);
            return events;
        }
        if let Some(result) = server_tool_result(&block) {
            self.lifecycle.step_start(&mut events);
            events.push(result);
        }
        events
    }

    fn content_block_delta(
        &mut self,
        index: Option<i64>,
        delta: Option<Delta>,
    ) -> Result<Vec<LlmEvent>, LlmError> {
        let mut events = Vec::new();
        let Some(delta) = delta else {
            return Ok(events);
        };
        let at = index.unwrap_or(0);
        let kind = delta.kind.as_deref();
        if kind == Some("text_delta")
            && let Some(text) = delta.text.as_deref().filter(|t| !t.is_empty())
        {
            self.lifecycle
                .text_delta(&mut events, &format!("text-{at}"), text);
            return Ok(events);
        }
        if kind == Some("thinking_delta")
            && let Some(text) = delta.thinking.as_deref().filter(|t| !t.is_empty())
        {
            self.lifecycle
                .reasoning_delta(&mut events, &format!("reasoning-{at}"), text, None);
            return Ok(events);
        }
        if kind == Some("signature_delta")
            && let Some(signature) = delta.signature.as_deref().filter(|t| !t.is_empty())
        {
            self.lifecycle.reasoning_end(
                &mut events,
                &format!("reasoning-{at}"),
                Some(meta("anthropic", "signature", signature)),
            );
            return Ok(events);
        }
        if kind == Some("input_json_delta")
            && let Some(index) = index
        {
            let Some(partial) = delta.partial_json.as_deref().filter(|t| !t.is_empty()) else {
                return Ok(events);
            };
            let appended = self.tools.append_existing(
                ADAPTER,
                index,
                partial,
                "Anthropic Messages tool argument delta is missing its tool call",
            )?;
            if !appended.is_empty() {
                self.lifecycle.step_start(&mut events);
            }
            events.extend(appended);
        }
        Ok(events)
    }

    fn content_block_stop(&mut self, index: Option<i64>) -> Result<Vec<LlmEvent>, LlmError> {
        let mut events = Vec::new();
        let Some(index) = index else {
            return Ok(events);
        };
        let finished = self.tools.finish(ADAPTER, &index, None)?;
        if finished.is_empty() {
            self.lifecycle
                .text_end(&mut events, &format!("text-{index}"), None);
            self.lifecycle
                .reasoning_end(&mut events, &format!("reasoning-{index}"), None);
        } else {
            self.lifecycle.step_start(&mut events);
        }
        events.extend(finished);
        Ok(events)
    }

    fn message_delta(&mut self, event: Event) -> Vec<LlmEvent> {
        self.usage = merge_usage(self.usage.take(), event.usage.map(map_usage));
        let delta = event.delta.unwrap_or_default();
        let mut events = Vec::new();
        self.lifecycle.finish(
            &mut events,
            map_finish_reason(delta.stop_reason.as_deref()),
            self.usage.as_ref().map(|(u, _)| u.clone()),
            delta
                .stop_sequence
                .filter(|s| !s.is_empty())
                .map(|s| meta("anthropic", "stopSequence", s)),
        );
        events
    }
}

/// JS array index from a JSON number (Anthropic indexes are small integers).
fn index_of(value: Option<f64>) -> Option<i64> {
    value.map(|v| v as i64)
}

impl ProtocolState for State {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let event: Event = decode_event(route, frame)?;
        let index = index_of(event.index);
        match event.kind.as_str() {
            "message_start" => {
                if let Some(usage) = event.message.and_then(|m| m.usage) {
                    self.usage = merge_usage(self.usage.take(), Some(map_usage(usage)));
                }
                Ok(vec![])
            }
            "content_block_start" => Ok(match event.content_block {
                Some(block) => self.content_block_start(index, block),
                None => vec![],
            }),
            "content_block_delta" => self.content_block_delta(index, event.delta),
            "content_block_stop" => self.content_block_stop(index),
            "message_delta" => Ok(self.message_delta(event)),
            "error" => {
                let message = provider_error_message(event.error.as_ref());
                let overflow = event
                    .error
                    .as_ref()
                    .and_then(|e| e.message.as_deref())
                    .is_some_and(is_context_overflow);
                Ok(vec![LlmEvent::ProviderError {
                    message,
                    classification: overflow
                        .then_some(ProviderFailureClassification::ContextOverflow),
                    retryable: None,
                    provider_metadata: None,
                }])
            }
            _ => Ok(vec![]),
        }
    }
}
