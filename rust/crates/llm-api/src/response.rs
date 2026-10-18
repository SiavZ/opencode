//! Purpose: assemble one provider attempt from ordered events (`LLMResponse.reduce/complete`).
//! Scope: pure fold.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/ids.rs
//! `-- llm-api/src/message.rs
//! External:
//! `-- indexmap::IndexMap
//! Used by:
//! rust/crates/
//! |-- llm-api/src/client.rs
//! `-- llm-api/src/lib.rs
//! Invariants: content order follows first appearance of each text/reasoning block id and
//! tool-call/result arrival; a response is complete only after `finish` or `provider-error`
//! (the latter sets finish reason `error` unless already set). Mirrors
//! packages/llm/src/schema/events.ts `reduceResponseState`.
//! Contract tests: llm-api/tests/contract.rs

use indexmap::IndexMap;

use crate::event::{LlmEvent, Usage};
use crate::ids::{FinishReason, ProviderMetadata};
use crate::message::{ContentPart, Message, ToolCallPart, ToolResultPart};

#[derive(Clone, Debug)]
struct Block {
    index: usize,
    text: String,
    meta: Option<ProviderMetadata>,
}

#[derive(Clone, Debug, Default)]
pub struct ToolInputAssembly {
    pub name: String,
    pub text: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

/// Accumulator for one provider attempt.
#[derive(Clone, Debug)]
pub struct ResponseState {
    pub events: Vec<LlmEvent>,
    pub message: Message,
    pub usage: Option<Usage>,
    pub finish_reason: Option<FinishReason>,
    text: IndexMap<String, Block>,
    reasoning: IndexMap<String, Block>,
    pub tool_inputs: IndexMap<String, ToolInputAssembly>,
}

impl Default for ResponseState {
    fn default() -> Self {
        Self {
            events: vec![],
            message: Message::assistant(vec![]),
            usage: None,
            finish_reason: None,
            text: IndexMap::new(),
            reasoning: IndexMap::new(),
            tool_inputs: IndexMap::new(),
        }
    }
}

fn text_part(text: &str, meta: &Option<ProviderMetadata>) -> ContentPart {
    ContentPart::Text {
        text: text.to_owned(),
        cache: None,
        metadata: None,
        provider_metadata: meta.clone(),
    }
}

fn reasoning_part(text: &str, meta: &Option<ProviderMetadata>) -> ContentPart {
    ContentPart::Reasoning {
        text: text.to_owned(),
        encrypted: None,
        metadata: None,
        provider_metadata: meta.clone(),
    }
}

impl ResponseState {
    /// Fold one event (`LLMResponse.reduce`).
    pub fn reduce(&mut self, event: LlmEvent) {
        match &event {
            LlmEvent::Finish { reason, usage, .. } => {
                if usage.is_some() {
                    self.usage = usage.clone();
                }
                self.finish_reason = Some(*reason);
            }
            LlmEvent::ProviderError { .. } => {
                self.finish_reason.get_or_insert(FinishReason::Error);
            }
            other => {
                if let Some(usage) = other.usage() {
                    self.usage = Some(usage.clone());
                }
            }
        }
        match &event {
            LlmEvent::TextStart {
                id,
                provider_metadata,
            } => {
                self.ensure(false, id, provider_metadata);
            }
            LlmEvent::TextDelta {
                id,
                text,
                provider_metadata,
            } => self.delta(false, id, text, provider_metadata),
            LlmEvent::TextEnd {
                id,
                provider_metadata,
            } => self.end(false, id, provider_metadata),
            LlmEvent::ReasoningStart {
                id,
                provider_metadata,
            } => {
                self.ensure(true, id, provider_metadata);
            }
            LlmEvent::ReasoningDelta {
                id,
                text,
                provider_metadata,
            } => self.delta(true, id, text, provider_metadata),
            LlmEvent::ReasoningEnd {
                id,
                provider_metadata,
            } => self.end(true, id, provider_metadata),
            LlmEvent::ToolInputStart {
                id,
                name,
                provider_metadata,
            } => {
                self.tool_inputs.insert(
                    id.clone(),
                    ToolInputAssembly {
                        name: name.clone(),
                        text: String::new(),
                        provider_metadata: provider_metadata.clone(),
                    },
                );
            }
            LlmEvent::ToolInputDelta { id, name, text } => {
                let entry =
                    self.tool_inputs
                        .entry(id.clone())
                        .or_insert_with(|| ToolInputAssembly {
                            name: name.clone(),
                            ..Default::default()
                        });
                entry.text.push_str(text);
            }
            LlmEvent::ToolInputEnd {
                id,
                name,
                provider_metadata,
            } => {
                let entry = self.tool_inputs.entry(id.clone()).or_default();
                entry.name = name.clone();
                if provider_metadata.is_some() {
                    entry.provider_metadata = provider_metadata.clone();
                }
            }
            LlmEvent::ToolCall {
                id,
                name,
                input,
                provider_executed,
                provider_metadata,
            } => {
                self.tool_inputs.shift_remove(id);
                self.message
                    .content
                    .push(ContentPart::ToolCall(ToolCallPart {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        provider_executed: *provider_executed,
                        metadata: None,
                        provider_metadata: provider_metadata.clone(),
                    }));
            }
            LlmEvent::ToolResult {
                id,
                name,
                result,
                provider_executed,
                provider_metadata,
                ..
            } => {
                self.message
                    .content
                    .push(ContentPart::ToolResult(ToolResultPart {
                        id: id.clone(),
                        name: name.clone(),
                        result: result.clone(),
                        provider_executed: *provider_executed,
                        cache: None,
                        metadata: None,
                        provider_metadata: provider_metadata.clone(),
                    }));
            }
            _ => {}
        }
        self.events.push(event);
    }

    fn blocks(&mut self, reasoning: bool) -> &mut IndexMap<String, Block> {
        if reasoning {
            &mut self.reasoning
        } else {
            &mut self.text
        }
    }

    fn ensure(&mut self, reasoning: bool, id: &str, meta: &Option<ProviderMetadata>) {
        if self.blocks(reasoning).contains_key(id) {
            return;
        }
        let index = self.message.content.len();
        let part = if reasoning {
            reasoning_part("", meta)
        } else {
            text_part("", meta)
        };
        self.message.content.push(part);
        self.blocks(reasoning).insert(
            id.to_owned(),
            Block {
                index,
                text: String::new(),
                meta: meta.clone(),
            },
        );
    }

    fn delta(&mut self, reasoning: bool, id: &str, text: &str, meta: &Option<ProviderMetadata>) {
        self.ensure(reasoning, id, meta);
        let Some(block) = self.blocks(reasoning).get_mut(id) else {
            return;
        };
        block.text.push_str(text);
        if meta.is_some() {
            block.meta = meta.clone();
        }
        let (index, part) = (block.index, block_part(reasoning, block));
        self.message.content[index] = part;
    }

    fn end(&mut self, reasoning: bool, id: &str, meta: &Option<ProviderMetadata>) {
        let Some(block) = self.blocks(reasoning).get_mut(id) else {
            return;
        };
        if meta.is_some() {
            block.meta = meta.clone();
        }
        let (index, part) = (block.index, block_part(reasoning, block));
        self.message.content[index] = part;
    }

    /// `LLMResponse.complete`: `Some` only after a terminal event.
    pub fn complete(self) -> Option<LlmResponse> {
        let finish_reason = self.finish_reason?;
        Some(LlmResponse {
            message: self.message,
            events: self.events,
            usage: self.usage,
            finish_reason,
        })
    }
}

fn block_part(reasoning: bool, block: &Block) -> ContentPart {
    if reasoning {
        reasoning_part(&block.text, &block.meta)
    } else {
        text_part(&block.text, &block.meta)
    }
}

/// `LLM.Response`: one completed provider attempt.
#[derive(Clone, Debug, PartialEq)]
pub struct LlmResponse {
    pub message: Message,
    pub events: Vec<LlmEvent>,
    pub usage: Option<Usage>,
    pub finish_reason: FinishReason,
}

impl LlmResponse {
    pub fn from_events(events: impl IntoIterator<Item = LlmEvent>) -> Option<Self> {
        let mut state = ResponseState::default();
        events.into_iter().for_each(|e| state.reduce(e));
        state.complete()
    }

    /// Concatenated `text-delta` text.
    pub fn text(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Concatenated `reasoning-delta` text.
    pub fn reasoning(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::ReasoningDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &LlmEvent> {
        self.events
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolCall { .. }))
    }
}
