//! Purpose: request option values (generation, HTTP overlay, provider options, limits,
//! defaults, cache hints/policy, compatibility) and their baseline merge precedence.
//! Scope: pure values and merge functions.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/ids.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/message.rs
//! `-- llm-api/src/route.rs
//! Invariants: merge semantics mirror packages/llm/src/schema/options.ts
//! (`mergeJsonRecords` deep-merges objects, later wins, undefined skipped, empty -> None).
//! Contract tests: llm-api/tests/contract.rs

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ids::JsonObject;

/// `Record<providerName, Record<string, unknown>>`.
pub type ProviderOptions = IndexMap<String, JsonObject>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOptions {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub max_tokens: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub temperature: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub top_p: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub top_k: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub frequency_penalty: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub presence_penalty: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub seed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

impl GenerationOptions {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
}

/// Selected-model request defaults, without applied precedence.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpOptions>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSchemaCompatibility {
    Gemini,
    Moonshot,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCompatibility {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_schema: Option<ToolSchemaCompatibility>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheHintType {
    Ephemeral,
    Persistent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHint {
    #[serde(rename = "type")]
    pub kind: CacheHintType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub ttl_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CacheMessagesPolicy {
    /// `"latest-user-message"` or `"latest-assistant"`.
    Named(String),
    Tail {
        tail: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachePolicyObject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<CacheMessagesPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::js_number"
    )]
    pub ttl_seconds: Option<f64>,
}

/// `"auto" | "none" | CachePolicyObject`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CachePolicy {
    Named(String),
    Object(CachePolicyObject),
}

/// `mergeJsonRecords`: deep merge of JSON objects, later keys win, nested objects merge.
/// `null` stands in for JS `undefined` only when the baseline would have dropped it, so
/// explicit JSON `null` values are kept (they are real values on the wire).
pub fn merge_json_records<'a>(
    items: impl IntoIterator<Item = Option<&'a JsonObject>>,
) -> Option<JsonObject> {
    let mut result = Map::new();
    let mut any = false;
    for item in items.into_iter().flatten() {
        any = true;
        for (key, value) in item {
            let merged = match (result.get(key), value) {
                (Some(Value::Object(prev)), Value::Object(next)) => {
                    Value::Object(merge_json_records([Some(prev), Some(next)]).unwrap_or_default())
                }
                _ => value.clone(),
            };
            result.insert(key.clone(), merged);
        }
    }
    if !any || result.is_empty() {
        return None;
    }
    Some(result)
}

fn merge_string_records<'a>(
    items: impl IntoIterator<Item = Option<&'a IndexMap<String, String>>>,
) -> Option<IndexMap<String, String>> {
    let defined: Vec<_> = items.into_iter().flatten().collect();
    if defined.is_empty() {
        return None;
    }
    let result: IndexMap<String, String> = defined
        .into_iter()
        .flat_map(|item| item.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect();
    (!result.is_empty()).then_some(result)
}

/// `mergeProviderOptions`: per-provider deep merge.
pub fn merge_provider_options<'a>(
    items: impl IntoIterator<Item = Option<&'a ProviderOptions>>,
) -> Option<ProviderOptions> {
    let mut result = ProviderOptions::new();
    for item in items.into_iter().flatten() {
        for (provider, options) in item {
            if let Some(merged) = merge_json_records([result.get(provider), Some(options)]) {
                result.insert(provider.clone(), merged);
            }
        }
    }
    (!result.is_empty()).then_some(result)
}

/// `mergeHttpOptions`.
pub fn merge_http_options<'a>(
    items: impl IntoIterator<Item = Option<&'a HttpOptions>> + Clone,
) -> Option<HttpOptions> {
    let body = merge_json_records(
        items
            .clone()
            .into_iter()
            .map(|i| i.and_then(|i| i.body.as_ref())),
    );
    let headers = merge_string_records(
        items
            .clone()
            .into_iter()
            .map(|i| i.and_then(|i| i.headers.as_ref())),
    );
    let query = merge_string_records(items.into_iter().map(|i| i.and_then(|i| i.query.as_ref())));
    if body.is_none() && headers.is_none() && query.is_none() {
        return None;
    }
    Some(HttpOptions {
        body,
        headers,
        query,
    })
}

/// `mergeGenerationOptions`: each field takes the last defined value.
pub fn merge_generation_options<'a>(
    items: impl IntoIterator<Item = Option<&'a GenerationOptions>>,
) -> Option<GenerationOptions> {
    let mut out = GenerationOptions::default();
    for item in items.into_iter().flatten() {
        macro_rules! take {
            ($($field:ident),*) => {$(if item.$field.is_some() { out.$field = item.$field.clone(); })*};
        }
        take!(
            max_tokens,
            temperature,
            top_p,
            top_k,
            frequency_penalty,
            presence_penalty,
            seed,
            stop
        );
    }
    (!out.is_empty()).then_some(out)
}
