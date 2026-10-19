//! Purpose: project ModelsDev reasoning_options without guessing controls for unsupported SDKs.
//! Scope: explicit effort, toggle and bounded token-budget options from legacy transform.ts.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/providers/variants.rs
//! `-- provider-adapters/src/providers/variants/efforts.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/variants.rs
//! Invariants: explicit empty reasoning options disable generated variants; budgets obey output limits.
//! Contract tests: provider-adapters/tests/providers_variants.rs

use super::efforts::{ENCRYPTED, adaptive, anthropic, modern, opus45, text};
use serde_json::{Map, Value, json};

pub(super) fn from_inventory(
    model: &Value,
    inventory: Option<&Value>,
) -> Option<Map<String, Value>> {
    let options = inventory?.get("reasoning_options")?.as_array()?;
    if options.is_empty() {
        return Some(Map::new());
    }
    if let Some(option) = options.iter().find(|option| option["type"] == "effort") {
        return Some(
            option
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|value| {
                    let id = if value.is_null() {
                        "none"
                    } else {
                        value.as_str()?
                    };
                    Some((id.to_owned(), effort(model, id)?))
                })
                .collect(),
        );
    }
    let mut result = Map::new();
    if options.iter().any(|option| option["type"] == "toggle") {
        match text(model, "/api/npm") {
            "@ai-sdk/alibaba" => result.extend([
                ("none".into(), json!({"enableThinking":false})),
                ("high".into(), json!({"enableThinking":true})),
            ]),
            "@ai-sdk/cohere" => result.extend([
                ("none".into(), json!({"thinking":{"type":"disabled"}})),
                ("high".into(), json!({"thinking":{"type":"enabled"}})),
            ]),
            _ => {}
        }
    }
    if let Some(option) = options
        .iter()
        .find(|option| option["type"] == "budget_tokens")
    {
        let output = model
            .pointer("/limit/output")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let maximum = option
            .get("max")
            .and_then(Value::as_i64)
            .unwrap_or(31999)
            .min(output - 1)
            .min(31999);
        if maximum > 0 {
            let high = option
                .get("min")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .max((maximum + 1) / 2)
                .min(maximum);
            for (id, tokens) in [("high", high), ("max", maximum)] {
                if let Some(value) = budget(model, tokens) {
                    result.insert(id.into(), value);
                }
            }
        }
    }
    (!result.is_empty()).then_some(result)
}

fn effort(model: &Value, value: &str) -> Option<Value> {
    let id = text(model, "/id");
    let api = text(model, "/api/id");
    Some(match text(model, "/api/npm") {
        "@openrouter/ai-sdk-provider" => json!({"reasoning":{"effort":value}}),
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => {
            anthropic(model, value).unwrap_or_else(|| json!({"effort":value}))
        }
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => {
            json!({"thinkingConfig":{"includeThoughts":true,"thinkingLevel":value}})
        }
        "@ai-sdk/amazon-bedrock" => {
            if adaptive(api).is_some() {
                let mut result =
                    json!({"reasoningConfig":{"type":"adaptive","maxReasoningEffort":value}});
                if modern(api) {
                    result["reasoningConfig"]["display"] = json!("summarized");
                }
                result
            } else if opus45(api) {
                let output = model
                    .pointer("/limit/output")
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                json!({"reasoningConfig":{"type":"enabled","budgetTokens":(16000.0_f64.min((output/2.0-1.0).floor())) as i64,"maxReasoningEffort":value}})
            } else if api.contains("anthropic") {
                return None;
            } else {
                json!({"reasoningConfig":{"type":"enabled","maxReasoningEffort":value}})
            }
        }
        "@ai-sdk/gateway" if id.contains("anthropic") => {
            json!({"thinking":{"type":"adaptive","display":"summarized"},"effort":value})
        }
        "@ai-sdk/gateway" if id.contains("google") => {
            json!({"thinkingConfig":{"includeThoughts":true,"thinkingLevel":value}})
        }
        "@ai-sdk/github-copilot" if id.contains("gemini") => return None,
        "@ai-sdk/github-copilot" if id.contains("claude") => json!({"reasoningEffort":value}),
        "@ai-sdk/openai"
        | "@ai-sdk/amazon-bedrock/mantle"
        | "@ai-sdk/azure"
        | "@ai-sdk/github-copilot" => {
            json!({"reasoningEffort":value,"reasoningSummary":"auto","include":ENCRYPTED})
        }
        "@jerome-benoit/sap-ai-provider-v2" if id.contains("anthropic") => {
            json!({"modelParams":{"thinking":{"type":"adaptive","display":"summarized"},"output_config":{"effort":value}}})
        }
        "@jerome-benoit/sap-ai-provider-v2" => json!({"modelParams":{"reasoning_effort":value}}),
        "@ai-sdk/gateway"
        | "@ai-sdk/openai-compatible"
        | "@ai-sdk/xai"
        | "@ai-sdk/mistral"
        | "@ai-sdk/groq"
        | "@ai-sdk/cerebras"
        | "@ai-sdk/deepinfra"
        | "@ai-sdk/togetherai"
        | "venice-ai-sdk-provider"
        | "ai-gateway-provider"
        | "merge-gateway-ai-sdk-provider" => json!({"reasoningEffort":value}),
        "gitlab-ai-provider" if text(model, "/family").starts_with("gpt") => {
            json!({"reasoningEffort":value})
        }
        "gitlab-ai-provider" if text(model, "/family").starts_with("claude") => {
            json!({"thinking":{"type":"adaptive","effort":value}})
        }
        _ => return None,
    })
}

fn budget(model: &Value, value: i64) -> Option<Value> {
    let id = text(model, "/id");
    Some(match text(model, "/api/npm") {
        "@openrouter/ai-sdk-provider" => json!({"reasoning":{"max_tokens":value}}),
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => {
            json!({"thinking":{"type":"enabled","budgetTokens":value}})
        }
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => {
            json!({"thinkingConfig":{"includeThoughts":true,"thinkingBudget":value}})
        }
        "@ai-sdk/amazon-bedrock" => {
            json!({"reasoningConfig":{"type":"enabled","budgetTokens":value}})
        }
        "@ai-sdk/gateway" if id.contains("anthropic") => {
            json!({"thinking":{"type":"enabled","budgetTokens":value}})
        }
        "@ai-sdk/gateway" if id.contains("google") => {
            json!({"thinkingConfig":{"includeThoughts":true,"thinkingBudget":value}})
        }
        "@ai-sdk/cohere" => json!({"thinking":{"type":"enabled","tokenBudget":value}}),
        "@ai-sdk/alibaba" => json!({"enableThinking":true,"thinkingBudget":value}),
        "@jerome-benoit/sap-ai-provider-v2" if id.contains("anthropic") => {
            json!({"modelParams":{"thinking":{"type":"enabled","budget_tokens":value}}})
        }
        "@jerome-benoit/sap-ai-provider-v2" if id.contains("gemini") => {
            json!({"modelParams":{"thinkingConfig":{"includeThoughts":true,"thinkingBudget":value}}})
        }
        _ => return None,
    })
}
