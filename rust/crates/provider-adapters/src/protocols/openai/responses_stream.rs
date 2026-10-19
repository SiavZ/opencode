//! Purpose: OpenAI Responses streaming state machine (packages/llm/src/protocols/openai-responses.ts
//! `step` and its `on*` handlers) and the `openai-responses` `Protocol` (shared by the HTTP and
//! WebSocket routes).
//! Scope: per-response state: text/reasoning blocks, reasoning summary parts, function-call
//! accumulation, hosted tools, finish and provider errors.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/openai/options.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! |-- provider-adapters/src/protocols/openai/responses_events.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! `-- provider-adapters/src/shared.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- options::store
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/protocols/openai/mod.rs
//! Invariants: `response.completed`/`response.incomplete` emit `finish`, `response.failed` and
//! `error` emit `provider-error`; the three `response.*` terminals end the stream. Summary
//! parts are keyed numerically (JS integer-key order). With `store: false` a finished summary
//! part stays open (`can-conclude`) until the next part starts or the item is done, so the last
//! `reasoning-end` carries the encrypted state.
//! Contract tests: provider-adapters/tests/openai_responses.rs

use std::collections::BTreeMap;

use indexmap::IndexMap;
use opencode_llm_api::{LlmError, LlmEvent, LlmRequest, ProviderMetadata};
use serde_json::{Map, Value};

use crate::protocol::{Frame, Protocol, ProtocolState};
use crate::protocols::openai::options;
use crate::protocols::openai::responses_body::from_request;
use crate::protocols::openai::responses_events::{
    ResponsesEvent, StreamItem, TERMINAL_TYPES, hosted_tool, hosted_tool_events, map_finish_reason,
    map_usage, provider_error,
};
use crate::protocols::openai::wire::{decode_event, flat, openai_metadata};
use crate::shared::{Lifecycle, PendingTool, ToolStream};

pub const ADAPTER: &str = "openai-responses";

/// `OpenAIResponses.protocol`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAIResponsesProtocol;

impl Protocol for OpenAIResponsesProtocol {
    fn id(&self) -> &'static str {
        ADAPTER
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        from_request(request)
    }

    fn initial(&self, request: &LlmRequest) -> Box<dyn ProtocolState> {
        Box::new(ResponsesState {
            store: options::store(request),
            ..ResponsesState::default()
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryStatus {
    Active,
    CanConclude,
    Concluded,
}

#[derive(Clone, Debug, Default)]
struct ReasoningItem {
    /// `undefined` (None), `null` (Some(None)) or the encrypted state.
    encrypted: Option<Option<String>>,
    parts: BTreeMap<u64, SummaryStatus>,
}

/// OpenAI Responses parser state.
#[derive(Debug, Default)]
pub struct ResponsesState {
    tools: ToolStream<String>,
    has_function_call: bool,
    lifecycle: Lifecycle,
    reasoning_items: IndexMap<String, ReasoningItem>,
    store: Option<bool>,
    terminal: bool,
}

fn metadata(pairs: &[(&str, Value)]) -> Option<ProviderMetadata> {
    Some(openai_metadata(Map::from_iter(
        pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())),
    )))
}

fn item_ref(id: &str) -> Option<ProviderMetadata> {
    metadata(&[("itemId", Value::from(id))])
}

fn reasoning_metadata(id: &str, encrypted: Option<&str>) -> Option<ProviderMetadata> {
    metadata(&[
        ("itemId", Value::from(id)),
        (
            "reasoningEncryptedContent",
            encrypted.map_or(Value::Null, Value::from),
        ),
    ])
}

/// JS template-literal rendering of a numeric summary index.
fn index_text(index: f64) -> String {
    if index.fract() == 0.0 && index.abs() < 1e21 {
        format!("{}", index as i64)
    } else {
        index.to_string()
    }
}

fn reasoning_item_id(item: &StreamItem) -> Option<&str> {
    (item.kind == "reasoning")
        .then_some(item.id.as_deref())
        .flatten()
        .filter(|id| !id.is_empty())
}

impl ResponsesState {
    fn reasoning_delta(&mut self, event: &ResponsesEvent) -> Vec<LlmEvent> {
        let Some(delta) = event.delta.as_deref().filter(|d| !d.is_empty()) else {
            return vec![];
        };
        let mut events = Vec::new();
        let item_id = event.item_id.as_deref().unwrap_or("reasoning-0");
        let id = if event.summary_index.is_some() || self.reasoning_items.contains_key(item_id) {
            format!(
                "{item_id}:{}",
                index_text(event.summary_index.unwrap_or(0.0))
            )
        } else {
            item_id.to_owned()
        };
        self.lifecycle
            .reasoning_delta(&mut events, &id, delta, None);
        events
    }

    fn output_item_added(&mut self, event: &ResponsesEvent) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        let Some(item) = &event.item else {
            return events;
        };
        if let Some(id) = reasoning_item_id(item) {
            let encrypted = flat(&item.encrypted_content).map(String::as_str);
            self.lifecycle.reasoning_start(
                &mut events,
                &format!("{id}:0"),
                reasoning_metadata(id, encrypted),
            );
            self.reasoning_items.insert(
                id.to_owned(),
                ReasoningItem {
                    encrypted: item.encrypted_content.clone(),
                    parts: BTreeMap::from([(0, SummaryStatus::Active)]),
                },
            );
            return events;
        }
        let Some(item_id) = item.id.as_deref().filter(|id| !id.is_empty()) else {
            return events;
        };
        if item.kind != "function_call" {
            return events;
        }
        let provider_metadata = item_ref(item_id);
        let call_id = item.call_id.clone().unwrap_or_else(|| item_id.to_owned());
        let name = item.name.clone().unwrap_or_default();
        self.lifecycle.step_start(&mut events);
        self.tools.start(
            item_id.to_owned(),
            PendingTool {
                id: call_id.clone(),
                name: name.clone(),
                input: item.arguments.clone().unwrap_or_default(),
                provider_executed: None,
                provider_metadata: provider_metadata.clone(),
            },
        );
        events.push(LlmEvent::ToolInputStart {
            id: call_id,
            name,
            provider_metadata,
        });
        events
    }

    fn summary_part_added(&mut self, event: &ResponsesEvent) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        let (Some(item_id), Some(index)) = (event.item_id.as_deref(), event.summary_index) else {
            return events;
        };
        let existing = self.reasoning_items.get(item_id).cloned();
        if index == 0.0 {
            if existing.is_some() {
                return events;
            }
            self.lifecycle.reasoning_start(
                &mut events,
                &format!("{item_id}:0"),
                reasoning_metadata(item_id, None),
            );
            self.reasoning_items.insert(
                item_id.to_owned(),
                ReasoningItem {
                    encrypted: None,
                    parts: BTreeMap::from([(0, SummaryStatus::Active)]),
                },
            );
            return events;
        }
        let mut item = existing.unwrap_or_default();
        for (key, status) in item.parts.iter_mut() {
            if *status == SummaryStatus::CanConclude {
                self.lifecycle.reasoning_end(
                    &mut events,
                    &format!("{item_id}:{key}"),
                    item_ref(item_id),
                );
                *status = SummaryStatus::Concluded;
            }
        }
        let encrypted = flat(&item.encrypted).map(String::as_str);
        self.lifecycle.reasoning_start(
            &mut events,
            &format!("{item_id}:{}", index_text(index)),
            reasoning_metadata(item_id, encrypted),
        );
        item.parts.insert(index as u64, SummaryStatus::Active);
        self.reasoning_items.insert(item_id.to_owned(), item);
        events
    }

    fn summary_part_done(&mut self, event: &ResponsesEvent) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        let (Some(item_id), Some(index)) = (event.item_id.as_deref(), event.summary_index) else {
            return events;
        };
        let Some(item) = self.reasoning_items.get_mut(item_id) else {
            return events;
        };
        let stored = self.store != Some(false);
        if stored {
            self.lifecycle.reasoning_end(
                &mut events,
                &format!("{item_id}:{}", index_text(index)),
                item_ref(item_id),
            );
        }
        let status = if stored {
            SummaryStatus::Concluded
        } else {
            SummaryStatus::CanConclude
        };
        item.parts.insert(index as u64, status);
        events
    }

    fn arguments_delta(&mut self, event: &ResponsesEvent) -> Result<Vec<LlmEvent>, LlmError> {
        let (Some(item_id), Some(delta)) = (
            event.item_id.as_deref(),
            event.delta.as_deref().filter(|d| !d.is_empty()),
        ) else {
            return Ok(vec![]);
        };
        let produced = self.tools.append_existing(
            ADAPTER,
            item_id.to_owned(),
            delta,
            "OpenAI Responses tool argument delta is missing its tool call",
        )?;
        let mut events = Vec::new();
        if !produced.is_empty() {
            self.lifecycle.step_start(&mut events);
        }
        events.extend(produced);
        Ok(events)
    }

    fn output_item_done(&mut self, event: &ResponsesEvent) -> Result<Vec<LlmEvent>, LlmError> {
        let mut events = Vec::new();
        let Some(item) = &event.item else {
            return Ok(events);
        };
        if item.kind == "function_call" {
            let non_empty = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
            let (Some(id), Some(call_id), Some(name)) = (
                non_empty(&item.id),
                non_empty(&item.call_id),
                non_empty(&item.name),
            ) else {
                return Ok(events);
            };
            if !self.tools.tools.contains_key(&id) {
                self.tools.start(
                    id.clone(),
                    PendingTool {
                        id: call_id,
                        name,
                        ..PendingTool::default()
                    },
                );
            }
            let produced = self.tools.finish(ADAPTER, &id, item.arguments.as_deref())?;
            if !produced.is_empty() {
                self.lifecycle.step_start(&mut events);
            }
            if produced
                .iter()
                .any(|e| matches!(e, LlmEvent::ToolCall { .. }))
            {
                self.has_function_call = true;
            }
            events.extend(produced);
            return Ok(events);
        }
        if let Some((name, input)) = hosted_tool(item) {
            self.lifecycle.step_start(&mut events);
            events.extend(hosted_tool_events(item, name, input));
            return Ok(events);
        }
        let Some(id) = reasoning_item_id(item) else {
            return Ok(events);
        };
        let encrypted = flat(&item.encrypted_content).map(String::as_str);
        let provider_metadata = reasoning_metadata(id, encrypted);
        if let Some(reasoning) = self.reasoning_items.shift_remove(id) {
            for (key, status) in &reasoning.parts {
                if matches!(status, SummaryStatus::Active | SummaryStatus::CanConclude) {
                    self.lifecycle.reasoning_end(
                        &mut events,
                        &format!("{id}:{key}"),
                        provider_metadata.clone(),
                    );
                }
            }
            return Ok(events);
        }
        if !self.lifecycle.reasoning.contains(id) {
            self.lifecycle.step_start(&mut events);
            events.push(LlmEvent::ReasoningStart {
                id: id.to_owned(),
                provider_metadata: provider_metadata.clone(),
            });
            events.push(LlmEvent::ReasoningEnd {
                id: id.to_owned(),
                provider_metadata,
            });
            return Ok(events);
        }
        self.lifecycle
            .reasoning_end(&mut events, id, provider_metadata);
        Ok(events)
    }

    fn response_finish(&mut self, event: &ResponsesEvent) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        let response = event.response.as_ref();
        let id = response.and_then(|r| r.id.clone());
        let tier = response.and_then(|r| r.service_tier.clone());
        let has_meta = id.as_deref().is_some_and(|s| !s.is_empty())
            || flat(&tier).is_some_and(|s| !s.is_empty());
        let provider_metadata = has_meta.then(|| {
            let mut map = Map::new();
            if let Some(id) = id {
                map.insert("responseId".into(), Value::from(id));
            }
            if let Some(tier) = tier {
                map.insert("serviceTier".into(), tier.map_or(Value::Null, Value::from));
            }
            openai_metadata(map)
        });
        let reason = map_finish_reason(event, self.has_function_call);
        let usage = map_usage(response.and_then(|r| flat(&r.usage)));
        self.lifecycle
            .finish(&mut events, reason, usage, provider_metadata);
        events
    }
}

impl ProtocolState for ResponsesState {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let event: ResponsesEvent = decode_event(route, &frame)?;
        if TERMINAL_TYPES.contains(&event.kind.as_str()) {
            self.terminal = true;
        }
        Ok(match event.kind.as_str() {
            "response.output_text.delta" => {
                let mut events = Vec::new();
                if let Some(delta) = event.delta.as_deref().filter(|d| !d.is_empty()) {
                    let id = event.item_id.as_deref().unwrap_or("text-0");
                    self.lifecycle.text_delta(&mut events, id, delta);
                }
                events
            }
            "response.reasoning_text.delta"
            | "response.reasoning_summary.delta"
            | "response.reasoning_summary_text.delta" => self.reasoning_delta(&event),
            "response.reasoning_summary_part.added" => self.summary_part_added(&event),
            "response.reasoning_summary_part.done" => self.summary_part_done(&event),
            "response.output_item.added" => self.output_item_added(&event),
            "response.function_call_arguments.delta" => self.arguments_delta(&event)?,
            "response.output_item.done" => self.output_item_done(&event)?,
            "response.completed" | "response.incomplete" => self.response_finish(&event),
            "response.failed" => vec![provider_error(&event, "OpenAI Responses response failed")],
            "error" => vec![provider_error(&event, "OpenAI Responses stream error")],
            _ => vec![],
        })
    }

    fn terminal(&self) -> bool {
        self.terminal
    }
}
