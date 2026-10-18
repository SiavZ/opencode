//! Purpose: Bedrock Converse stream state machine (bedrock-converse.ts `step` and `onHalt`)
//! over AWS event-stream records projected to `{ eventType: payload }`: tool-use start/deltas,
//! text and reasoningContent deltas with per-block signatures, messageStop/metadata finish
//! consolidation, usage mapping, and stream exceptions as `provider-error` events.
//! Scope: pure parser over decoded frames.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/transport/overflow.rs
//! External:
//! `-- serde::Deserialize
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/bedrock/mod.rs
//! Invariants: exactly one finish, emitted on halt after both `messageStop` and `metadata` had a
//! chance to arrive; `stop` with completed tool calls becomes `tool-calls`.
//! Contract tests: provider-adapters/tests/bedrock_converse.rs

use std::collections::HashMap;

use opencode_llm_api::{FinishReason, LlmError, LlmEvent, ProviderFailureClassification, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::protocol::{Frame, ProtocolState};
use crate::protocols::anthropic::wire::{decode_event, meta};
use crate::shared::{Lifecycle, PendingTool, ToolStream, subtract_tokens, total_tokens};
use crate::transport::is_context_overflow;

const ADAPTER: &str = "bedrock-converse";
const USAGE_KEYS: [&str; 5] = [
    "inputTokens",
    "outputTokens",
    "totalTokens",
    "cacheReadInputTokens",
    "cacheWriteInputTokens",
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolUseStart {
    tool_use_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockStartBody {
    #[serde(default)]
    tool_use: Option<ToolUseStart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockStart {
    content_block_index: f64,
    #[serde(default)]
    start: Option<BlockStartBody>,
}

#[derive(Deserialize)]
struct ToolUseDelta {
    input: String,
}

#[derive(Deserialize)]
struct ReasoningDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeltaBody {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_use: Option<ToolUseDelta>,
    #[serde(default)]
    reasoning_content: Option<ReasoningDelta>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockDelta {
    content_block_index: f64,
    #[serde(default)]
    delta: Option<DeltaBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockStop {
    content_block_index: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageStop {
    stop_reason: String,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    usage: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
struct Exception {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    #[serde(default)]
    content_block_start: Option<BlockStart>,
    #[serde(default)]
    content_block_delta: Option<BlockDelta>,
    #[serde(default)]
    content_block_stop: Option<BlockStop>,
    #[serde(default)]
    message_stop: Option<MessageStop>,
    #[serde(default)]
    metadata: Option<Metadata>,
    #[serde(default)]
    internal_server_exception: Option<Exception>,
    #[serde(default)]
    model_stream_error_exception: Option<Exception>,
    #[serde(default)]
    validation_exception: Option<Exception>,
    #[serde(default)]
    throttling_exception: Option<Exception>,
    #[serde(default)]
    service_unavailable_exception: Option<Exception>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UsageCounts {
    #[serde(default)]
    input_tokens: Option<f64>,
    #[serde(default)]
    output_tokens: Option<f64>,
    #[serde(default)]
    total_tokens: Option<f64>,
    #[serde(default)]
    cache_read_input_tokens: Option<f64>,
    #[serde(default)]
    cache_write_input_tokens: Option<f64>,
}

#[derive(Default)]
pub(crate) struct State {
    tools: ToolStream<i64>,
    pending_finish: Option<(FinishReason, Option<Usage>)>,
    has_tool_calls: bool,
    lifecycle: Lifecycle,
    reasoning_signatures: HashMap<i64, String>,
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        "content_filtered" | "guardrail_intervened" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

/// `mapUsage`: `inputTokens` is inclusive of cache reads and writes.
fn map_usage(raw: Map<String, Value>) -> Result<Usage, serde_json::Error> {
    let counts: UsageCounts = serde_json::from_value(Value::Object(raw.clone()))?;
    let cache_total = counts.cache_read_input_tokens.unwrap_or(0.0)
        + counts.cache_write_input_tokens.unwrap_or(0.0);
    let projected: Map<String, Value> = USAGE_KEYS
        .iter()
        .filter_map(|k| raw.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    Ok(Usage {
        input_tokens: counts.input_tokens,
        output_tokens: counts.output_tokens,
        non_cached_input_tokens: subtract_tokens(counts.input_tokens, Some(cache_total)),
        cache_read_input_tokens: counts.cache_read_input_tokens,
        cache_write_input_tokens: counts.cache_write_input_tokens,
        reasoning_tokens: None,
        total_tokens: total_tokens(
            counts.input_tokens,
            counts.output_tokens,
            counts.total_tokens,
        ),
        provider_metadata: Some([("bedrock".to_owned(), projected)].into_iter().collect()),
    })
}

fn provider_error(
    message: String,
    classification: Option<ProviderFailureClassification>,
    retryable: bool,
) -> Vec<LlmEvent> {
    vec![LlmEvent::ProviderError {
        message,
        classification,
        retryable: Some(retryable),
        provider_metadata: None,
    }]
}

impl ProtocolState for State {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let raw = frame.raw();
        let event: Event = decode_event(route, frame)?;
        let mut events = Vec::new();

        if let Some(start) = &event.content_block_start
            && let Some(tool) = start.start.as_ref().and_then(|s| s.tool_use.as_ref())
        {
            self.lifecycle.step_start(&mut events);
            self.tools.start(
                start.content_block_index as i64,
                PendingTool {
                    id: tool.tool_use_id.clone(),
                    name: tool.name.clone(),
                    ..Default::default()
                },
            );
            events.push(LlmEvent::ToolInputStart {
                id: tool.tool_use_id.clone(),
                name: tool.name.clone(),
                provider_metadata: None,
            });
            return Ok(events);
        }

        if let Some(block) = &event.content_block_delta
            && let Some(delta) = &block.delta
        {
            let index = block.content_block_index as i64;
            if let Some(text) = delta.text.as_deref().filter(|t| !t.is_empty()) {
                self.lifecycle
                    .text_delta(&mut events, &format!("text-{index}"), text);
                return Ok(events);
            }
            if let Some(reasoning) = &delta.reasoning_content {
                if let Some(text) = reasoning.text.as_deref().filter(|t| !t.is_empty()) {
                    self.lifecycle.reasoning_delta(
                        &mut events,
                        &format!("reasoning-{index}"),
                        text,
                        None,
                    );
                }
                if let Some(signature) = reasoning.signature.as_ref().filter(|s| !s.is_empty()) {
                    self.reasoning_signatures.insert(index, signature.clone());
                }
                return Ok(events);
            }
            if let Some(tool) = &delta.tool_use {
                let appended = self.tools.append_existing(
                    ADAPTER,
                    index,
                    &tool.input,
                    "Bedrock Converse tool delta is missing its tool call",
                )?;
                if !appended.is_empty() {
                    self.lifecycle.step_start(&mut events);
                }
                events.extend(appended);
                return Ok(events);
            }
        }

        if let Some(stop) = &event.content_block_stop {
            let index = stop.content_block_index as i64;
            let finished = self.tools.finish(ADAPTER, &index, None)?;
            if finished.is_empty() {
                self.lifecycle
                    .text_end(&mut events, &format!("text-{index}"), None);
                let signature = self
                    .reasoning_signatures
                    .get(&index)
                    .map(|s| meta("bedrock", "signature", s.clone()));
                self.lifecycle
                    .reasoning_end(&mut events, &format!("reasoning-{index}"), signature);
            } else {
                self.lifecycle.step_start(&mut events);
            }
            if finished
                .iter()
                .any(|e| matches!(e, LlmEvent::ToolCall { .. }))
            {
                self.has_tool_calls = true;
            }
            events.extend(finished);
            self.reasoning_signatures.remove(&index);
            return Ok(events);
        }

        if let Some(stop) = &event.message_stop {
            let usage = self.pending_finish.take().and_then(|(_, u)| u);
            self.pending_finish = Some((map_finish_reason(&stop.stop_reason), usage));
            return Ok(events);
        }

        if let Some(metadata) = event.metadata {
            let usage =
                match metadata.usage {
                    Some(raw_usage) => Some(map_usage(raw_usage).map_err(|_| {
                        crate::protocols::anthropic::wire::invalid_event(route, raw)
                    })?),
                    None => None,
                };
            let reason = self
                .pending_finish
                .as_ref()
                .map_or(FinishReason::Stop, |(r, _)| *r);
            self.pending_finish = Some((reason, usage));
            return Ok(events);
        }

        if let Some(e) = event
            .internal_server_exception
            .or(event.model_stream_error_exception)
            .or(event.service_unavailable_exception)
        {
            return Ok(provider_error(e.message, None, true));
        }

        let throttled = event.throttling_exception.is_some();
        if let Some(validation) = event.validation_exception {
            let overflow = is_context_overflow(&validation.message);
            return Ok(provider_error(
                validation.message,
                overflow.then_some(ProviderFailureClassification::ContextOverflow),
                throttled,
            ));
        }
        if let Some(throttling) = event.throttling_exception {
            return Ok(provider_error(throttling.message, None, true));
        }
        Ok(events)
    }

    fn on_halt(&mut self, _route: &str) -> Result<Vec<LlmEvent>, LlmError> {
        let Some((reason, usage)) = self.pending_finish.take() else {
            return Ok(vec![]);
        };
        let reason = if reason == FinishReason::Stop && self.has_tool_calls {
            FinishReason::ToolCalls
        } else {
            reason
        };
        let mut events = Vec::new();
        self.lifecycle.finish(&mut events, reason, usage, None);
        Ok(events)
    }
}
