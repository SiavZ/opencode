//! Purpose: Gemini SSE stream state machine (gemini.ts `step` and `onHalt`): candidate parts to
//! text/reasoning deltas, thought signatures on reasoning-end and tool calls, sequential
//! `tool_N` function-call ids, usageMetadata mapping and finishReason (including safety
//! reasons) mapping on halt.
//! Scope: pure parser over decoded frames.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! `-- provider-adapters/src/shared.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde_json::Error
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::from_value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/gemini/mod.rs
//! Invariants: the terminal finish is emitted once on halt, only if a finishReason or usage was
//! seen; `STOP` with tool calls maps to `tool-calls`.
//! Contract tests: provider-adapters/tests/gemini.rs

use opencode_llm_api::{FinishReason, LlmError, LlmEvent, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::protocol::{Frame, ProtocolState};
use crate::protocols::anthropic::wire::{decode_event, meta};
use crate::shared::{Lifecycle, subtract_tokens, total_tokens};

const USAGE_KEYS: [&str; 5] = [
    "cachedContentTokenCount",
    "thoughtsTokenCount",
    "promptTokenCount",
    "candidatesTokenCount",
    "totalTokenCount",
];

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct UsageCounts {
    #[serde(default)]
    cached_content_token_count: Option<f64>,
    #[serde(default)]
    thoughts_token_count: Option<f64>,
    #[serde(default)]
    prompt_token_count: Option<f64>,
    #[serde(default)]
    candidates_token_count: Option<f64>,
    #[serde(default)]
    total_token_count: Option<f64>,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

/// One content part; the TS union is matched by key presence.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thought: Option<bool>,
    #[serde(default)]
    thought_signature: Option<String>,
    #[serde(default)]
    function_call: Option<FunctionCall>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    #[serde(default)]
    candidates: Option<Vec<Candidate>>,
    #[serde(default)]
    usage_metadata: Option<Map<String, Value>>,
}

#[derive(Default)]
pub(crate) struct State {
    finish_reason: Option<String>,
    has_tool_calls: bool,
    next_tool_call_id: u64,
    usage: Option<Usage>,
    lifecycle: Lifecycle,
    reasoning_signature: Option<String>,
}

/// `mapUsage`: prompt is inclusive of cached; candidates exclude thoughts.
fn map_usage(raw: Map<String, Value>) -> Result<Usage, serde_json::Error> {
    let counts: UsageCounts = serde_json::from_value(Value::Object(raw.clone()))?;
    let cached = counts.cached_content_token_count;
    let output = counts
        .candidates_token_count
        .map(|c| c + counts.thoughts_token_count.unwrap_or(0.0));
    let projected: Map<String, Value> = USAGE_KEYS
        .iter()
        .filter_map(|k| raw.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    Ok(Usage {
        input_tokens: counts.prompt_token_count,
        output_tokens: output,
        non_cached_input_tokens: subtract_tokens(counts.prompt_token_count, cached),
        cache_read_input_tokens: cached,
        cache_write_input_tokens: None,
        reasoning_tokens: counts.thoughts_token_count,
        total_tokens: total_tokens(counts.prompt_token_count, output, counts.total_token_count),
        provider_metadata: Some([("google".to_owned(), projected)].into_iter().collect()),
    })
}

fn map_finish_reason(reason: Option<&str>, has_tool_calls: bool) -> FinishReason {
    match reason {
        Some("STOP") if has_tool_calls => FinishReason::ToolCalls,
        Some("STOP") => FinishReason::Stop,
        Some("MAX_TOKENS") => FinishReason::Length,
        Some(
            "IMAGE_SAFETY" | "RECITATION" | "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII",
        ) => FinishReason::ContentFilter,
        Some("MALFORMED_FUNCTION_CALL") => FinishReason::Error,
        _ => FinishReason::Unknown,
    }
}

impl State {
    fn signature_meta(&self) -> Option<opencode_llm_api::ProviderMetadata> {
        self.reasoning_signature
            .clone()
            .map(|s| meta("google", "thoughtSignature", s))
    }
}

impl ProtocolState for State {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let raw = frame.raw();
        let event: Event = decode_event(route, frame)?;
        if let Some(usage) = event.usage_metadata {
            self.usage = Some(
                map_usage(usage)
                    .map_err(|_| crate::protocols::anthropic::wire::invalid_event(route, raw))?,
            );
        }
        let Some(candidate) = event.candidates.and_then(|c| c.into_iter().next()) else {
            return Ok(vec![]);
        };
        if candidate.finish_reason.is_some() {
            self.finish_reason = candidate.finish_reason;
        }
        let Some(content) = candidate.content else {
            return Ok(vec![]);
        };
        let mut events = Vec::new();
        for part in content.parts {
            if part.thought == Some(true)
                && let Some(sig) = part.thought_signature.as_ref().filter(|s| !s.is_empty())
            {
                self.reasoning_signature = Some(sig.clone());
            }
            if let Some(text) = part.text.as_deref().filter(|t| !t.is_empty()) {
                if part.thought == Some(true) {
                    let meta = part
                        .thought_signature
                        .filter(|s| !s.is_empty())
                        .map(|s| meta("google", "thoughtSignature", s));
                    self.lifecycle
                        .reasoning_delta(&mut events, "reasoning-0", text, meta);
                    continue;
                }
                let signature = self.signature_meta();
                self.lifecycle
                    .reasoning_end(&mut events, "reasoning-0", signature);
                self.lifecycle.text_delta(&mut events, "text-0", text);
                continue;
            }
            if let Some(call) = part.function_call {
                let id = format!("tool_{}", self.next_tool_call_id);
                self.next_tool_call_id += 1;
                let signature = self.signature_meta();
                self.lifecycle
                    .reasoning_end(&mut events, "reasoning-0", signature);
                self.lifecycle.step_start(&mut events);
                events.push(LlmEvent::ToolCall {
                    id,
                    name: call.name,
                    input: call.args,
                    provider_executed: None,
                    provider_metadata: part
                        .thought_signature
                        .filter(|s| !s.is_empty())
                        .map(|s| meta("google", "thoughtSignature", s)),
                });
                self.has_tool_calls = true;
            }
        }
        Ok(events)
    }

    fn on_halt(&mut self, _route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        if self.finish_reason.is_none() && self.usage.is_none() {
            return Ok(vec![]);
        }
        let mut events = Vec::new();
        if self.reasoning_signature.is_some() {
            let meta = self.signature_meta();
            self.lifecycle
                .reasoning_end(&mut events, "reasoning-0", meta);
        }
        self.lifecycle.finish(
            &mut events,
            map_finish_reason(self.finish_reason.as_deref(), self.has_tool_calls),
            self.usage.clone(),
            None,
        );
        Ok(events)
    }
}
