//! Purpose: ordered provider-neutral stream events (`LLM.Event` union) and token usage.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/ids.rs
//! `-- llm-api/src/message.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- llm-api/src/client.rs
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/response.rs
//! `-- llm-api/src/route.rs
//! Invariants: `type` tags and camelCase fields match packages/llm/src/schema/events.ts.
//! A well-formed stream ends with exactly one `finish` or `provider-error` terminal event.
//! Provider-executed tool calls carry `providerExecuted: true` and must never be dispatched
//! to local tools by consumers.
//! Contract tests: llm-api/tests/contract.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProviderFailureClassification;
use crate::ids::{FinishReason, ProviderMetadata, js_number};
use crate::message::{ToolOutput, ToolResultValue};

/// `LLM.Usage`. Inclusive totals plus a non-overlapping breakdown:
/// `nonCachedInputTokens + cacheReadInputTokens + cacheWriteInputTokens = inputTokens` and
/// `reasoningTokens <= outputTokens`. Protocol mappers derive whichever side is missing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub non_cached_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub cache_read_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub cache_write_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub reasoning_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub total_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl Usage {
    /// `visibleOutputTokens`: output minus reasoning, clamped at zero.
    pub fn visible_output_tokens(&self) -> f64 {
        (self.output_tokens.unwrap_or(0.0) - self.reasoning_tokens.unwrap_or(0.0)).max(0.0)
    }
}

/// `ProviderShared.totalTokens`: provider total wins, else input+output when either is known.
pub fn total_tokens(input: Option<f64>, output: Option<f64>, total: Option<f64>) -> Option<f64> {
    if total.is_some() {
        return total;
    }
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(input.unwrap_or(0.0) + output.unwrap_or(0.0))
}

/// `ProviderShared.subtractTokens`: clamped difference, `None` when total unknown.
pub fn subtract_tokens(total: Option<f64>, subtrahend: Option<f64>) -> Option<f64> {
    let total = total?;
    Some(subtrahend.map_or(total, |s| (total - s).max(0.0)))
}

/// `LLM.Event` tagged union, in stream order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LlmEvent {
    StepStart {
        index: u32,
    },
    #[serde(rename_all = "camelCase")]
    TextStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    TextDelta {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    TextEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningDelta {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ToolInputStart {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolInputDelta {
        id: String,
        name: String,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolInputEnd {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        id: String,
        name: String,
        result: ToolResultValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<ToolOutput>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ToolError {
        id: String,
        name: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    StepFinish {
        index: u32,
        reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    Finish {
        reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    ProviderError {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classification: Option<ProviderFailureClassification>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl LlmEvent {
    /// Wire discriminator (`type`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::StepStart { .. } => "step-start",
            Self::TextStart { .. } => "text-start",
            Self::TextDelta { .. } => "text-delta",
            Self::TextEnd { .. } => "text-end",
            Self::ReasoningStart { .. } => "reasoning-start",
            Self::ReasoningDelta { .. } => "reasoning-delta",
            Self::ReasoningEnd { .. } => "reasoning-end",
            Self::ToolInputStart { .. } => "tool-input-start",
            Self::ToolInputDelta { .. } => "tool-input-delta",
            Self::ToolInputEnd { .. } => "tool-input-end",
            Self::ToolCall { .. } => "tool-call",
            Self::ToolResult { .. } => "tool-result",
            Self::ToolError { .. } => "tool-error",
            Self::StepFinish { .. } => "step-finish",
            Self::Finish { .. } => "finish",
            Self::ProviderError { .. } => "provider-error",
        }
    }

    /// `finish` and `provider-error` close a provider attempt.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finish { .. } | Self::ProviderError { .. })
    }

    pub fn usage(&self) -> Option<&Usage> {
        match self {
            Self::StepFinish { usage, .. } | Self::Finish { usage, .. } => usage.as_ref(),
            _ => None,
        }
    }
}
