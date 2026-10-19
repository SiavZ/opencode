//! Purpose: generate exact legacy reasoning variant tables from provider/transform.ts.
//! Scope: pure V1 public model JSON; V2 inventory variants remain configured separately.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/providers/variants/efforts.rs
//! `-- provider-adapters/src/providers/variants/reasoning.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- runtime/src/provider_services/legacy.rs
//! `-- provider-adapters/tests/providers_variants.rs
//! Invariants: non-reasoning and unsupported SDK models never invent reasoning controls.
//! Contract tests: provider-adapters/tests/providers_variants.rs

mod efforts;
mod reasoning;
use efforts::{
    ALL, ENCRYPTED, WIDE, adaptive, anthropic, gemini25, google, kimi, modern, openai, opus45, text,
};
use serde_json::{Map, Value, json};

pub fn generated(model: &Value, inventory: Option<&Value>) -> Map<String, Value> {
    reasoning::from_inventory(model, inventory).unwrap_or_else(|| variants(model))
}

fn settings(values: &[&str], build: impl Fn(&str) -> Value) -> Map<String, Value> {
    values
        .iter()
        .map(|effort| ((*effort).to_owned(), build(effort)))
        .collect()
}

pub fn variants(model: &Value) -> Map<String, Value> {
    if model
        .pointer("/capabilities/reasoning")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Map::new();
    }
    let id = text(model, "/id").to_lowercase();
    let api = text(model, "/api/id");
    let package = text(model, "/api/npm");
    let provider = text(model, "/providerID");
    let release = text(model, "/release_date");
    let glm52 = ["glm-5.2", "glm-5-2", "glm-5p2"]
        .iter()
        .any(|name| id.contains(name) || api.to_lowercase().contains(name));
    if api.to_lowercase().contains("minimax-m3")
        && ["@ai-sdk/anthropic", "@ai-sdk/openai-compatible"].contains(&package)
    {
        return if ["nvidia", "lilac"].contains(&provider) {
            [
                (
                    "none".into(),
                    json!({"chat_template_kwargs":{"thinking_mode":"disabled"}}),
                ),
                (
                    "thinking".into(),
                    json!({"chat_template_kwargs":{"thinking_mode":"enabled"}}),
                ),
            ]
            .into_iter()
            .collect()
        } else {
            [
                ("none".into(), json!({"thinking":{"type":"disabled"}})),
                ("thinking".into(), json!({"thinking":{"type":"adaptive"}})),
            ]
            .into_iter()
            .collect()
        };
    }
    if glm52 {
        match package {
            "@openrouter/ai-sdk-provider" => {
                return settings(
                    &["high", "xhigh"],
                    |effort| json!({"reasoning":{"effort":effort}}),
                );
            }
            "@ai-sdk/openai-compatible" => {
                return settings(&["high", "max"], |effort| json!({"reasoningEffort":effort}));
            }
            "@ai-sdk/anthropic" => {
                return settings(&["high", "max"], |effort| json!({"effort":effort}));
            }
            _ => {}
        }
    }
    if kimi(model) && ["@ai-sdk/anthropic", "@ai-sdk/google-vertex/anthropic"].contains(&package) {
        return settings(
            &["low", "medium", "high", "xhigh", "max"],
            |effort| json!({"thinking":{"type":"adaptive","display":"summarized"},"effort":effort}),
        );
    }
    if [
        "deepseek-chat",
        "deepseek-reasoner",
        "deepseek-r1",
        "deepseek-v3",
        "minimax",
        "kimi",
        "k2p",
        "qwen",
        "big-pickle",
    ]
    .iter()
    .any(|name| id.contains(name))
        || (id.contains("glm") && !glm52)
    {
        return Map::new();
    }
    if id.contains("grok-3-mini") {
        return settings(&["low", "high"], |effort| {
            if package == "@openrouter/ai-sdk-provider" {
                json!({"reasoning":{"effort":effort}})
            } else {
                json!({"reasoningEffort":effort})
            }
        });
    }
    match package {
        "@openrouter/ai-sdk-provider" => settings(
            &if api.starts_with("openai/") || id.contains("gpt") {
                openai(api, release, true)
            } else {
                WIDE.to_vec()
            },
            |effort| json!({"reasoning":{"effort":effort}}),
        ),
        "ai-gateway-provider" => settings(
            &if api.starts_with("openai/") {
                openai(api, release, false)
            } else {
                WIDE.to_vec()
            },
            |effort| json!({"reasoningEffort":effort}),
        ),
        "@ai-sdk/gateway" => gateway(model),
        "@ai-sdk/github-copilot" => {
            if text(model, "/id").contains("gemini") {
                return Map::new();
            }
            if text(model, "/id").contains("claude") {
                return settings(WIDE, |effort| json!({"reasoningEffort":effort}));
            }
            let mut values = WIDE.to_vec();
            if ["5.1-codex-max", "5.2", "5.3"]
                .iter()
                .any(|name| id.contains(name))
                || (id.contains("gpt-5") && release >= "2025-12-04")
            {
                values.push("xhigh");
            }
            settings(
                &values,
                |effort| json!({"reasoningEffort":effort,"reasoningSummary":"auto","include":ENCRYPTED}),
            )
        }
        "@ai-sdk/cerebras"
        | "@ai-sdk/togetherai"
        | "@ai-sdk/xai"
        | "@ai-sdk/deepinfra"
        | "venice-ai-sdk-provider"
        | "@ai-sdk/openai-compatible" => {
            let values = if api.to_lowercase().contains("north-mini-code") {
                vec!["none", "high"]
            } else if api.to_lowercase().contains("deepseek-v4") {
                vec!["low", "medium", "high", "max"]
            } else {
                WIDE.to_vec()
            };
            settings(&values, |effort| json!({"reasoningEffort":effort}))
        }
        "@ai-sdk/azure" => {
            if id == "o1-mini" {
                return Map::new();
            }
            settings(
                &openai(&id, release, false),
                |effort| json!({"reasoningEffort":effort,"reasoningSummary":"auto","include":ENCRYPTED}),
            )
        }
        "@ai-sdk/openai" | "@ai-sdk/amazon-bedrock/mantle" => settings(
            &if provider == "meta" {
                ALL.to_vec()
            } else {
                openai(api, release, false)
            },
            |effort| json!({"reasoningEffort":effort,"reasoningSummary":"auto","include":ENCRYPTED}),
        ),
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => {
            if let Some(values) = adaptive(api) {
                let values: Vec<_> = values
                    .into_iter()
                    .filter(|effort| {
                        provider != "github-copilot"
                            || ((*effort != "max" && *effort != "xhigh")
                                && (!api.contains("opus-4.7") || *effort == "medium"))
                    })
                    .collect();
                return settings(&values, |effort| {
                    anthropic(model, effort).expect("adaptive family")
                });
            }
            if opus45(api) {
                return settings(WIDE, |effort| {
                    anthropic(model, effort).expect("opus 4.5 family")
                });
            }
            let output = model
                .pointer("/limit/output")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            [("high".into(),json!({"thinking":{"type":"enabled","budgetTokens":(16000.0_f64.min((output/2.0-1.0).floor())) as i64}})),("max".into(),json!({"thinking":{"type":"enabled","budgetTokens":(31999.0_f64.min(output-1.0)) as i64}}))].into_iter().collect()
        }
        "@ai-sdk/amazon-bedrock" => {
            if let Some(values) = adaptive(api) {
                return settings(&values, |effort| {
                    let mut value =
                        json!({"reasoningConfig":{"type":"adaptive","maxReasoningEffort":effort}});
                    if modern(api) {
                        value["reasoningConfig"]["display"] = json!("summarized");
                    }
                    value
                });
            }
            if api.contains("anthropic") {
                return [
                    (
                        "high".into(),
                        json!({"reasoningConfig":{"type":"enabled","budgetTokens":16000}}),
                    ),
                    (
                        "max".into(),
                        json!({"reasoningConfig":{"type":"enabled","budgetTokens":31999}}),
                    ),
                ]
                .into_iter()
                .collect();
            }
            settings(
                WIDE,
                |effort| json!({"reasoningConfig":{"type":"enabled","maxReasoningEffort":effort}}),
            )
        }
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => google(model),
        "@ai-sdk/mistral"
            if [
                "mistral-small-2603",
                "mistral-small-latest",
                "mistral-medium-3.5",
                "mistral-medium-2604",
            ]
            .iter()
            .any(|name| api.to_lowercase().contains(name)) =>
        {
            settings(&["high"], |effort| json!({"reasoningEffort":effort}))
        }
        "@ai-sdk/groq" => settings(
            &["none", "low", "medium", "high"],
            |effort| json!({"reasoningEffort":effort}),
        ),
        "@jerome-benoit/sap-ai-provider-v2" => sap(model),
        _ => Map::new(),
    }
}

fn gateway(model: &Value) -> Map<String, Value> {
    let api = text(model, "/api/id");
    if api.contains("anthropic") {
        if let Some(values) = adaptive(api) {
            return settings(&values, |effort| {
                anthropic(model, effort).expect("adaptive family")
            });
        }
        return [
            (
                "high".into(),
                json!({"thinking":{"type":"enabled","budgetTokens":16000}}),
            ),
            (
                "max".into(),
                json!({"thinking":{"type":"enabled","budgetTokens":31999}}),
            ),
        ]
        .into_iter()
        .collect();
    }
    if api.contains("google") {
        if gemini25(api) {
            return google(model);
        }
        return settings(
            &["low", "high"],
            |effort| json!({"includeThoughts":true,"thinkingLevel":effort}),
        );
    }
    settings(
        &openai(api, text(model, "/release_date"), true),
        |effort| json!({"reasoningEffort":effort}),
    )
}

fn sap(model: &Value) -> Map<String, Value> {
    let id = text(model, "/id").to_lowercase();
    let api = text(model, "/api/id");
    if id.contains("anthropic") {
        if let Some(values) = adaptive(api) {
            return settings(&values, |effort| {
                let mut value = json!({"modelParams":{"thinking":{"type":"adaptive"},"output_config":{"effort":effort}}});
                if modern(api) {
                    value["modelParams"]["thinking"]["display"] = json!("summarized");
                }
                value
            });
        }
        return [
            (
                "high".into(),
                json!({"modelParams":{"thinking":{"type":"enabled","budget_tokens":16000}}}),
            ),
            (
                "max".into(),
                json!({"modelParams":{"thinking":{"type":"enabled","budget_tokens":31999}}}),
            ),
        ]
        .into_iter()
        .collect();
    }
    if gemini25(&id) || gemini25(api) {
        return google(model)
            .into_iter()
            .map(|(id, value)| (id, json!({"modelParams":value})))
            .collect();
    }
    let values = if id.contains("gpt")
        || regex::Regex::new(r"\bo[1-9]")
            .expect("static SAP model predicate")
            .is_match(&id)
    {
        openai(&id, text(model, "/release_date"), false)
    } else {
        WIDE.to_vec()
    };
    settings(
        &values,
        |effort| json!({"modelParams":{"reasoning_effort":effort}}),
    )
}
