//! Purpose: OpenAI Chat streaming decoder (packages/llm/src/protocols/openai-chat.ts `step`,
//! `finishEvents`, `mapUsage`, `mapFinishReason`): chunk schema validation, text and
//! `reasoning_content` deltas, indexed tool-call accumulation and usage mapping.
//! Scope: per-response state machine.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! `-- provider-adapters/src/shared.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value::Object
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! `-- provider-adapters/src/protocols/openai/mod.rs
//! Invariants: only the first choice is read. Each non-empty `finish_reason` replaces the
//! previous one, and the first one finalizes all pending tool calls at that chunk. Tool-call
//! events are emitted on halt before `finish`, and `stop` with tool calls reports `tool-calls`.
//! Later usage chunks replace earlier usage. Usage provider metadata holds only the
//! schema-known keys in schema order.
//! Contract tests: provider-adapters/tests/openai_chat.rs

use opencode_llm_api::{FinishReason, LlmError, LlmEvent, Usage};
use serde::{Deserialize, Serialize};

use crate::protocol::{Frame, ProtocolState};
use crate::protocols::openai::chat::ADAPTER;
use crate::protocols::openai::wire::{
    decode_event, flat, nullable, openai_metadata, present, ser_js_num,
};
use crate::shared::{Lifecycle, ToolStream, subtract_tokens, total_tokens};

#[derive(Debug, Deserialize, Serialize)]
struct CachedDetails {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    cached_tokens: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReasoningDetails {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    reasoning_tokens: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatUsage {
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    prompt_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    completion_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "present::deserialize",
        serialize_with = "ser_js_num",
        skip_serializing_if = "Option::is_none"
    )]
    total_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    prompt_tokens_details: Option<Option<CachedDetails>>,
    #[serde(
        default,
        deserialize_with = "nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    completion_tokens_details: Option<Option<ReasoningDetails>>,
}

#[derive(Debug, Deserialize)]
struct ToolFunctionDelta {
    #[serde(default, deserialize_with = "nullable::deserialize")]
    name: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    arguments: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    index: f64,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    id: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    function: Option<Option<ToolFunctionDelta>>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(default, deserialize_with = "nullable::deserialize")]
    content: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    reasoning_content: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    tool_calls: Option<Option<Vec<ToolCallDelta>>>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default, deserialize_with = "nullable::deserialize")]
    delta: Option<Option<Delta>>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    finish_reason: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
struct ChatEvent {
    choices: Vec<Choice>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    usage: Option<Option<ChatUsage>>,
}

/// `mapFinishReason`.
pub fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        "function_call" | "tool_calls" => FinishReason::ToolCalls,
        _ => FinishReason::Unknown,
    }
}

fn map_usage(usage: &ChatUsage) -> Usage {
    let cached = flat(&usage.prompt_tokens_details).and_then(|d| d.cached_tokens);
    let reasoning = flat(&usage.completion_tokens_details).and_then(|d| d.reasoning_tokens);
    let metadata = match serde_json::to_value(usage) {
        Ok(serde_json::Value::Object(map)) => Some(openai_metadata(map)),
        _ => None,
    };
    Usage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        non_cached_input_tokens: subtract_tokens(usage.prompt_tokens, cached),
        cache_read_input_tokens: cached,
        cache_write_input_tokens: None,
        reasoning_tokens: reasoning,
        total_tokens: total_tokens(
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
        ),
        provider_metadata: metadata,
    }
}

/// OpenAI Chat parser state.
#[derive(Debug, Default)]
pub struct ChatState {
    tools: ToolStream<u64>,
    tool_call_events: Vec<LlmEvent>,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
    lifecycle: Lifecycle,
}

impl ProtocolState for ChatState {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let event: ChatEvent = decode_event(route, &frame)?;
        let mut events = Vec::new();
        if let Some(usage) = flat(&event.usage) {
            self.usage = Some(map_usage(usage));
        }
        let choice = event.choices.first();
        let previous_finish = self.finish_reason;
        // A later non-empty finish_reason replaces the earlier one (OpenRouter repeats it).
        if let Some(reason) = choice
            .and_then(|c| flat(&c.finish_reason))
            .filter(|r| !r.is_empty())
        {
            self.finish_reason = Some(map_finish_reason(reason));
        }
        let delta = choice.and_then(|c| flat(&c.delta));
        let non_empty = |v: &Option<Option<String>>| flat(v).filter(|s| !s.is_empty()).cloned();
        if let Some(text) = delta.and_then(|d| non_empty(&d.reasoning_content)) {
            self.lifecycle
                .reasoning_delta(&mut events, "reasoning-0", &text, None);
        }
        if let Some(text) = delta.and_then(|d| non_empty(&d.content)) {
            self.lifecycle
                .reasoning_end(&mut events, "reasoning-0", None);
            self.lifecycle.text_delta(&mut events, "text-0", &text);
        }
        let tool_deltas = delta.and_then(|d| flat(&d.tool_calls));
        if tool_deltas.is_some_and(|t| !t.is_empty()) {
            self.lifecycle
                .reasoning_end(&mut events, "reasoning-0", None);
        }
        for tool in tool_deltas.into_iter().flatten() {
            let function = flat(&tool.function);
            let produced = self.tools.append_or_start(
                ADAPTER,
                tool.index as u64,
                flat(&tool.id).map(String::as_str),
                function.and_then(|f| flat(&f.name)).map(String::as_str),
                function
                    .and_then(|f| flat(&f.arguments))
                    .map_or("", String::as_str),
                "OpenAI Chat tool call delta is missing id or name",
            )?;
            if !produced.is_empty() {
                self.lifecycle.step_start(&mut events);
            }
            events.extend(produced);
        }
        // Finalize accumulated tool inputs eagerly when finish_reason arrives so JSON parse
        // failures fail the stream at the boundary rather than at halt.
        if self.finish_reason.is_some() && previous_finish.is_none() && !self.tools.tools.is_empty()
        {
            self.tool_call_events = self.tools.finish_all(ADAPTER)?;
        }
        Ok(events)
    }

    fn on_halt(&mut self, _route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        let mut events = Vec::new();
        let has_tool_calls = !self.tool_call_events.is_empty();
        let reason = match self.finish_reason {
            Some(FinishReason::Stop) if has_tool_calls => Some(FinishReason::ToolCalls),
            other => other,
        };
        if has_tool_calls {
            self.lifecycle.step_start(&mut events);
        }
        events.append(&mut self.tool_call_events);
        if let Some(reason) = reason {
            self.lifecycle
                .finish(&mut events, reason, self.usage.clone(), None);
        }
        Ok(events)
    }
}
