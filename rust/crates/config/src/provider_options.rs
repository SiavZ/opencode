//! Purpose: provider option lowerers (`packages/core/src/v1/config/provider-options.ts`) mapping
//! V1 AI SDK options into V2 provider settings, headers, body and per-request bodies.
//! Scope: pure transforms.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- config/src/lib.rs
//! `-- config/src/migrate.rs
//! Invariants: package-to-lowerer table and key handling match the baseline exactly; unknown
//! packages use the raw lowerer (`{ body: options }`).
//! Contract tests: config/tests/current.rs

use serde_json::{Map, Value};

type Options = Map<String, Value>;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProviderResult {
    pub headers: Option<Options>,
    pub body: Option<Options>,
    pub url: Option<String>,
    pub settings: Option<Options>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowerer {
    Raw,
    OpenAi,
    Anthropic,
    Google,
    Azure,
    Bedrock,
    OpenAiCompatible,
}

pub fn get(package: Option<&str>) -> Lowerer {
    match package.unwrap_or("") {
        "@ai-sdk/openai" => Lowerer::OpenAi,
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => Lowerer::Anthropic,
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => Lowerer::Google,
        "@ai-sdk/azure" => Lowerer::Azure,
        "@ai-sdk/amazon-bedrock" => Lowerer::Bedrock,
        "@ai-sdk/openai-compatible"
        | "@ai-sdk/cerebras"
        | "@ai-sdk/deepinfra"
        | "@ai-sdk/groq"
        | "@ai-sdk/mistral"
        | "@ai-sdk/togetherai"
        | "@ai-sdk/xai"
        | "@openrouter/ai-sdk-provider"
        | "ai-gateway-provider"
        | "venice-ai-sdk-provider" => Lowerer::OpenAiCompatible,
        _ => Lowerer::Raw,
    }
}

impl Lowerer {
    pub fn provider(self, options: &Options) -> ProviderResult {
        match self {
            Self::Raw => ProviderResult {
                body: Some(options.clone()),
                ..Default::default()
            },
            Self::OpenAi => ProviderResult {
                url: string(options.get("baseURL")),
                headers: compact(
                    vec![
                        ("Authorization", bearer(options.get("apiKey"))),
                        ("OpenAI-Organization", string(options.get("organization"))),
                        ("OpenAI-Project", string(options.get("project"))),
                    ],
                    options.get("headers"),
                ),
                body: body(options.get("body")),
                settings: Some(omit(
                    options,
                    &[
                        "apiKey",
                        "baseURL",
                        "organization",
                        "project",
                        "headers",
                        "body",
                    ],
                )),
            },
            Self::Anthropic => ProviderResult {
                url: string(options.get("baseURL")),
                headers: compact(
                    vec![
                        ("x-api-key", string(options.get("apiKey"))),
                        (
                            "Authorization",
                            if truthy(options.get("authToken")) {
                                bearer(options.get("authToken"))
                            } else {
                                None
                            },
                        ),
                    ],
                    options.get("headers"),
                ),
                body: body(options.get("body")),
                settings: Some(omit(
                    options,
                    &["apiKey", "authToken", "baseURL", "headers", "body"],
                )),
            },
            Self::Google | Self::Azure => {
                let key = if self == Self::Google {
                    "x-goog-api-key"
                } else {
                    "api-key"
                };
                ProviderResult {
                    url: string(options.get("baseURL")),
                    headers: compact(
                        vec![(key, string(options.get("apiKey")))],
                        options.get("headers"),
                    ),
                    body: body(options.get("body")),
                    settings: Some(omit(options, &["apiKey", "baseURL", "headers", "body"])),
                }
            }
            Self::Bedrock => direct(options, &[]),
            Self::OpenAiCompatible => ProviderResult {
                url: string(options.get("baseURL")),
                ..direct(options, &["baseURL"])
            },
        }
    }

    pub fn request(self, options: &Options) -> Options {
        match self {
            Self::Raw => options.clone(),
            Self::OpenAi | Self::Azure => openai_request(options),
            Self::Anthropic => {
                let mut result = snake(options);
                if options.contains_key("effort") || options.contains_key("taskBudget") {
                    let mut config = Options::new();
                    for (from, to) in [("effort", "effort"), ("taskBudget", "task_budget")] {
                        if let Some(value) = options.get(from) {
                            config.insert(to.into(), value.clone());
                        }
                    }
                    result.insert("output_config".into(), Value::Object(config));
                    result.remove("effort");
                    result.remove("task_budget");
                }
                if let Some(user) = options.get("metadata").and_then(|m| m.get("userId")) {
                    let mut metadata = result
                        .get("metadata")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    metadata.insert("user_id".into(), user.clone());
                    result.insert("metadata".into(), Value::Object(metadata));
                }
                result
            }
            Self::Google => {
                const KEYS: &[&str] = &[
                    "thinkingConfig",
                    "responseModalities",
                    "mediaResolution",
                    "imageConfig",
                ];
                let generation: Options = options
                    .iter()
                    .filter(|(key, _)| KEYS.contains(&key.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let mut result = omit(options, KEYS);
                if !generation.is_empty() {
                    result.insert("generationConfig".into(), Value::Object(generation));
                }
                result
            }
            Self::Bedrock => Options::from_iter([(
                "additionalModelRequestFields".into(),
                Value::Object(options.clone()),
            )]),
            Self::OpenAiCompatible => {
                let mut result = options.clone();
                if let Some(effort) = options.get("reasoningEffort") {
                    result.insert("reasoning_effort".into(), effort.clone());
                    result.remove("reasoningEffort");
                }
                result
            }
        }
    }
}

fn openai_request(options: &Options) -> Options {
    let mut result = snake(options);
    if options.contains_key("reasoningEffort") || options.contains_key("reasoningSummary") {
        let mut reasoning = result
            .get("reasoning")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(effort) = options.get("reasoningEffort") {
            reasoning.insert("effort".into(), effort.clone());
        }
        if let Some(summary) = options.get("reasoningSummary") {
            reasoning.insert("summary".into(), summary.clone());
        }
        result.insert("reasoning".into(), Value::Object(reasoning));
        result.remove("reasoning_effort");
        result.remove("reasoning_summary");
    }
    if let Some(verbosity) = options.get("textVerbosity") {
        let mut text = result
            .get("text")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        text.insert("verbosity".into(), verbosity.clone());
        result.insert("text".into(), Value::Object(text));
        result.remove("text_verbosity");
    }
    result
}

fn direct(options: &Options, extra: &[&str]) -> ProviderResult {
    let mut keys = vec!["headers", "body"];
    keys.extend_from_slice(extra);
    ProviderResult {
        headers: headers(options.get("headers")),
        body: body(options.get("body")),
        settings: Some(omit(options, &keys)),
        url: None,
    }
}

fn body(input: Option<&Value>) -> Option<Options> {
    input.and_then(Value::as_object).cloned()
}

fn headers(input: Option<&Value>) -> Option<Options> {
    let map = input?.as_object()?;
    Some(
        map.iter()
            .filter(|(_, value)| value.is_string())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

fn compact(entries: Vec<(&str, Option<String>)>, extra: Option<&Value>) -> Option<Options> {
    let mut map = Options::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.into(), Value::String(value));
        }
    }
    for (key, value) in headers(extra).unwrap_or_default() {
        map.insert(key, value);
    }
    (!map.is_empty()).then_some(map)
}

fn omit(options: &Options, keys: &[&str]) -> Options {
    options
        .iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => true,
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn bearer(value: Option<&Value>) -> Option<String> {
    string(value).map(|token| format!("Bearer {token}"))
}

fn snake_key(key: &str) -> String {
    key.chars().fold(String::new(), |mut out, ch| {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
        out
    })
}

fn snake_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(snake_value).collect()),
        Value::Object(map) => Value::Object(snake(map)),
        other => other.clone(),
    }
}

fn snake(options: &Options) -> Options {
    options
        .iter()
        .map(|(key, value)| (snake_key(key), snake_value(value)))
        .collect()
}
