//! Purpose: read OpenAI provider options from `request.providerOptions.openai`
//! (packages/llm/src/protocols/utils/openai-options.ts): store, reasoning effort and summary,
//! include, prompt cache key, text verbosity, service tier, instructions.
//! Scope: pure option readers. Unknown or mistyped values read as absent.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! `-- provider-adapters/src/protocols/openai/responses_stream.rs
//! Invariants: `include` drops entries outside the SDK `ResponseIncludable` union and an
//! empty result reads as absent. `reasoning_effort` accepts every canonical effort, callers
//! reject `max` with `is_openai_reasoning_effort`.
//! Contract tests: provider-adapters/tests/openai_responses.rs

use opencode_llm_api::{JsonObject, LlmRequest};
use serde_json::Value;

/// Canonical `ReasoningEfforts` (schema/ids.ts).
pub const REASONING_EFFORTS: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
/// `OpenAIReasoningEfforts`: every canonical effort except `max`.
pub const OPENAI_REASONING_EFFORTS: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh"];
/// `OpenAIResponseIncludables`, in lockstep with openai-node `ResponseIncludable`.
pub const RESPONSE_INCLUDABLES: &[&str] = &[
    "file_search_call.results",
    "web_search_call.results",
    "web_search_call.action.sources",
    "message.input_image.image_url",
    "computer_call_output.output.image_url",
    "code_interpreter_call.outputs",
    "reasoning.encrypted_content",
    "message.output_text.logprobs",
];
pub const SERVICE_TIERS: &[&str] = &["auto", "default", "flex", "priority"];
const TEXT_VERBOSITY: &[&str] = &["low", "medium", "high"];

fn options(request: &LlmRequest) -> Option<&JsonObject> {
    request.provider_options.as_ref()?.get("openai")
}

fn string<'a>(request: &'a LlmRequest, key: &str) -> Option<&'a str> {
    options(request)?.get(key)?.as_str()
}

fn one_of<'a>(request: &'a LlmRequest, key: &str, allowed: &[&str]) -> Option<&'a str> {
    string(request, key).filter(|v| allowed.contains(v))
}

pub fn store(request: &LlmRequest) -> Option<bool> {
    options(request)?.get("store")?.as_bool()
}

pub fn reasoning_effort(request: &LlmRequest) -> Option<&str> {
    one_of(request, "reasoningEffort", REASONING_EFFORTS)
}

pub fn is_openai_reasoning_effort(effort: &str) -> bool {
    OPENAI_REASONING_EFFORTS.contains(&effort)
}

pub fn reasoning_summary(request: &LlmRequest) -> Option<&'static str> {
    (string(request, "reasoningSummary") == Some("auto")).then_some("auto")
}

pub fn include(request: &LlmRequest) -> Option<Vec<String>> {
    let Value::Array(values) = options(request)?.get("include")? else {
        return None;
    };
    let filtered: Vec<String> = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|v| RESPONSE_INCLUDABLES.contains(v))
        .map(str::to_owned)
        .collect();
    (!filtered.is_empty()).then_some(filtered)
}

pub fn prompt_cache_key(request: &LlmRequest) -> Option<&str> {
    string(request, "promptCacheKey")
}

pub fn text_verbosity(request: &LlmRequest) -> Option<&str> {
    one_of(request, "textVerbosity", TEXT_VERBOSITY)
}

pub fn service_tier(request: &LlmRequest) -> Option<&str> {
    one_of(request, "serviceTier", SERVICE_TIERS)
}

pub fn instructions(request: &LlmRequest) -> Option<&str> {
    string(request, "instructions")
}
