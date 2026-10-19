//! Purpose: provider request-transform rules from packages/opencode/src/provider/transform.ts that
//! are pure functions of the model: sampling defaults (`temperature`, `topP`, `topK`), output cap
//! (`maxOutputTokens`), provider-options namespacing (`sdkKey`, `providerOptions` incl. gateway
//! slugs, Azure dual keys, OpenAI reasoning gate and Anthropic block binding), `smallOptions` and
//! `sanitizeSurrogates`.
//! Scope: pure functions over `TransformModel`. Message normalization, variants and schema
//! rewriting are not here (see providers-blockers.md).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- llm-api/src/options.rs
//! External:
//! |-- char::decode_utf16
//! |-- regex::Regex
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/providers/transform_message.rs
//! |-- provider-adapters/src/providers/transform_message/media.rs
//! |-- provider-adapters/tests/providers_transform.rs
//! `-- provider-adapters/tests/providers_transform_messages.rs
//! Invariants: outputs match the baseline object shapes and key order.
//! Contract tests: provider-adapters/tests/providers_transform.rs

use std::sync::LazyLock;

use opencode_llm_api::JsonObject;
use regex::Regex;
use serde_json::{Value, json};

pub const OUTPUT_TOKEN_MAX: u64 = 32_000;

/// The `Provider.Model` fields the transform rules read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformModel {
    pub provider_id: String,
    pub api_id: String,
    pub npm: String,
    pub url: String,
    pub output_limit: u64,
    pub reasoning: bool,
    pub input_modalities: Vec<String>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

static GEMINI_SAMPLING: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(r"(?i)gemini-2[.-]5(?:[.-]|$)"),
        re(r"gemini-3-(?:flash|pro)(?:[.-]|$)"),
        re(r"gemini-3[.-]1(?:[.-]|$)"),
    ]
});
/// `gemini-3[.-]5-flash(?!-lite)(?:[.-]|$)`: the look-ahead is checked by hand.
static GEMINI_35_FLASH: LazyLock<Regex> = LazyLock::new(|| re(r"gemini-3[.-]5-flash"));
static GEMINI_LEGACY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)gemini-(?:(?:flash|pro)-)?[12](?:[.-]|$)"));
static CLAUDE_BINDING: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)claude-(?:([a-z]+)-)?(\d+)(?:[.-](\d{1,2}))?(?:-([a-z]+))?(?:[.@-]|$)")
});

fn gemini_sampling(id: &str) -> bool {
    GEMINI_SAMPLING.iter().any(|r| r.is_match(id))
        || GEMINI_35_FLASH.find_iter(id).any(|m| {
            let rest = &id[m.end()..];
            !rest.starts_with("-lite")
                && (rest.is_empty() || rest.starts_with('.') || rest.starts_with('-'))
        })
}

pub fn is_legacy_gemini(api_id: &str) -> bool {
    GEMINI_LEGACY.is_match(api_id)
}

/// `temperature(model)`.
pub fn temperature(model: &TransformModel) -> Option<f64> {
    let id = model.api_id.to_lowercase();
    if id.contains("north-mini-code") {
        return Some(1.0);
    }
    if id.contains("claude") {
        return None;
    }
    if id.contains("gemini") {
        return gemini_sampling(&id).then_some(1.0);
    }
    if ["glm-4.6", "glm-4.7", "minimax-m2"]
        .iter()
        .any(|s| id.contains(s))
    {
        return Some(1.0);
    }
    if id.contains("kimi-k2") {
        if ["thinking", "k2.", "k2p", "k2-5"]
            .iter()
            .any(|s| id.contains(s))
        {
            return Some(1.0);
        }
        return Some(0.6);
    }
    None
}

/// `topP(model)`.
pub fn top_p(model: &TransformModel) -> Option<f64> {
    let id = model.api_id.to_lowercase();
    if id.contains("gemini") {
        return gemini_sampling(&id).then_some(0.95);
    }
    if ["minimax-m2", "kimi-k2.5", "kimi-k2p5", "kimi-k2-5"]
        .iter()
        .any(|s| id.contains(s))
    {
        return Some(0.95);
    }
    let flash_0731 = ["deepseek-v4-flash-0731", "deepseek-v4-flash:0731"]
        .iter()
        .any(|s| id.contains(s));
    let flash = id.contains("deepseek-v4-flash")
        && (model.provider_id == "deepseek" || model.provider_id.starts_with("opencode"));
    (flash_0731 || flash).then_some(0.95)
}

/// `topK(model)`.
pub fn top_k(model: &TransformModel) -> Option<f64> {
    let id = model.api_id.to_lowercase();
    if id.contains("minimax-m2") {
        if ["m2.", "m25", "m21"].iter().any(|s| id.contains(s)) {
            return Some(40.0);
        }
        return Some(20.0);
    }
    if id.contains("gemini") {
        return gemini_sampling(&id).then_some(64.0);
    }
    None
}

/// `maxOutputTokens(model, outputTokenMax)`: `min(limit, max) || max`.
pub fn max_output_tokens(model: &TransformModel, output_token_max: u64) -> u64 {
    match model.output_limit.min(output_token_max) {
        0 => output_token_max,
        n => n,
    }
}

/// `sanitizeSurrogates`. Rust strings cannot hold lone surrogates, so this applies to
/// UTF-16 input (e.g. decoded JS strings) and returns valid text with U+FFFD replacements.
pub fn sanitize_surrogates(units: &[u16]) -> String {
    char::decode_utf16(units.iter().copied())
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect()
}

/// `sdkKey(npm)`: the providerOptions namespace the AI SDK package reads.
pub fn sdk_key(npm: &str) -> Option<&'static str> {
    Some(match npm {
        "@ai-sdk/github-copilot" => "copilot",
        "@ai-sdk/azure" => "azure",
        "@ai-sdk/openai" | "@ai-sdk/amazon-bedrock/mantle" => "openai",
        "@ai-sdk/amazon-bedrock" => "bedrock",
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => "anthropic",
        "@ai-sdk/google-vertex" => "vertex",
        "@ai-sdk/google" => "google",
        "@ai-sdk/alibaba" => "alibaba",
        "@ai-sdk/cerebras" => "cerebras",
        "@ai-sdk/cohere" => "cohere",
        "@ai-sdk/deepinfra" => "deepinfra",
        "@ai-sdk/groq" => "groq",
        "@ai-sdk/mistral" => "mistral",
        "@ai-sdk/perplexity" => "perplexity",
        "@ai-sdk/togetherai" => "togetherai",
        "@ai-sdk/vercel" => "vercel",
        "@ai-sdk/xai" => "xai",
        "venice-ai-sdk-provider" => "venice",
        "@ai-sdk/gateway" => "gateway",
        "@openrouter/ai-sdk-provider" => "openrouter",
        "merge-gateway-ai-sdk-provider" => "mergeGateway",
        "ai-gateway-provider" => "openaiCompatible",
        _ => return None,
    })
}

/// `anthropicBindsThinking(apiId)`: Claude 5.1+ except Mythos 5.1.
pub fn anthropic_binds_thinking(api_id: &str) -> bool {
    let Some(c) = CLAUDE_BINDING.captures(api_id) else {
        return false;
    };
    let major: u64 = c[2].parse().unwrap_or(0);
    let minor: u64 = c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let family = c.get(1).or(c.get(4)).map(|m| m.as_str().to_lowercase());
    if major == 5 && minor == 1 && family.as_deref() == Some("mythos") {
        return false;
    }
    major > 5 || (major == 5 && minor >= 1)
}

/// `anthropicBlockBinding(model, options)`.
pub fn anthropic_block_binding(model: &TransformModel, options: &JsonObject) -> JsonObject {
    let key = match sdk_key(&model.npm) {
        Some("bedrock") => Some("reasoningConfig"),
        Some("anthropic") => Some("thinking"),
        _ => None,
    };
    if let Some(key) = key
        && let Some(Value::Object(inner)) = options.get(key)
        && inner.get("blockBinding") == Some(&Value::Bool(false))
    {
        let mut result = options.clone();
        let mut inner = inner.clone();
        inner.shift_remove("blockBinding");
        if inner.is_empty() {
            result.shift_remove(key);
        } else {
            result.insert(key.into(), Value::Object(inner));
        }
        return result;
    }
    if !anthropic_binds_thinking(&model.api_id) {
        return options.clone();
    }
    let key = match model.npm.as_str() {
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => "thinking",
        "@ai-sdk/amazon-bedrock" => "reasoningConfig",
        _ => return options.clone(),
    };
    let current = options
        .get(key)
        .cloned()
        .unwrap_or_else(|| json!({ "type": "adaptive" }));
    let kind = current.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("adaptive" | "enabled")) || current.get("blockBinding").is_some() {
        return options.clone();
    }
    let mut next = current.as_object().cloned().unwrap_or_default();
    next.insert(
        "blockBinding".into(),
        json!({ "prefixMismatchBehavior": "drop_block" }),
    );
    let mut result = options.clone();
    result.insert(key.into(), Value::Object(next));
    result
}

/// `providerOptions(model, options)`: wrap options under the SDK namespace.
pub fn provider_options(model: &TransformModel, options: &JsonObject) -> JsonObject {
    let openai_gate = matches!(
        model.npm.as_str(),
        "@ai-sdk/openai" | "@ai-sdk/azure" | "@ai-sdk/amazon-bedrock/mantle"
    );
    let normalized = if openai_gate
        && (model.reasoning
            || options.contains_key("reasoningEffort")
            || options.contains_key("reasoningSummary"))
    {
        let mut o = options.clone();
        o.insert("forceReasoning".into(), Value::Bool(true));
        o
    } else {
        anthropic_block_binding(model, options)
    };
    if model.npm == "@ai-sdk/gateway" {
        return gateway_options(model, normalized);
    }
    let dot_split = matches!(
        model.npm.as_str(),
        "@ai-sdk/openai-compatible" | "@ai-sdk/openai" | "@ai-sdk/anthropic"
    );
    let key = match sdk_key(&model.npm) {
        Some(k) => k.to_owned(),
        None if dot_split => model
            .provider_id
            .split('.')
            .next()
            .unwrap_or_default()
            .to_owned(),
        None => model.provider_id.clone(),
    };
    let mut out = JsonObject::new();
    if model.npm == "@ai-sdk/azure" {
        out.insert("openai".into(), Value::Object(normalized.clone()));
        out.insert("azure".into(), Value::Object(normalized));
        return out;
    }
    out.insert(key, Value::Object(normalized));
    out
}

fn gateway_options(model: &TransformModel, mut normalized: JsonObject) -> JsonObject {
    let slug = model
        .api_id
        .find('/')
        .filter(|i| *i > 0)
        .map(|i| &model.api_id[..i])
        .map(|s| if s == "amazon" { "bedrock" } else { s });
    let gateway = normalized.shift_remove("gateway");
    let mut result = JsonObject::new();
    if let Some(g) = &gateway {
        result.insert("gateway".into(), g.clone());
    }
    if normalized.is_empty() {
        return result;
    }
    match (slug, gateway) {
        (Some(slug), _) => {
            result.insert(slug.into(), Value::Object(normalized));
        }
        (None, Some(Value::Object(mut g))) => {
            g.extend(normalized);
            result.insert("gateway".into(), Value::Object(g));
        }
        (None, _) => {
            result.insert("gateway".into(), Value::Object(normalized));
        }
    }
    result
}

/// `smallOptions(model)`: `small` is the first variant's options (or `{}`).
pub fn small_options(model: &TransformModel, first_variant: Option<&JsonObject>) -> JsonObject {
    let small = first_variant.cloned().unwrap_or_default();
    if model.provider_id == "openai"
        || matches!(
            model.npm.as_str(),
            "@ai-sdk/openai" | "@ai-sdk/github-copilot" | "@ai-sdk/xai"
        )
    {
        let base = JsonObject::from_iter([("store".to_owned(), Value::Bool(false))]);
        return opencode_llm_api::options::merge_json_records([Some(&base), Some(&small)])
            .unwrap_or_default();
    }
    if (model.provider_id == "openrouter" || model.provider_id == "llmgateway")
        && small.is_empty()
        && model.api_id.contains("google")
    {
        return json!({ "reasoning": { "enabled": false } })
            .as_object()
            .cloned()
            .unwrap_or_default();
    }
    if model.provider_id == "venice" && small.is_empty() {
        return json!({ "veniceParameters": { "disableThinking": true } })
            .as_object()
            .cloned()
            .unwrap_or_default();
    }
    small
}
