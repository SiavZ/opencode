//! Purpose: small JSON wire helpers shared by the Anthropic, Gemini and Bedrock protocol
//! families: JS-number rendering, ordered object building with `undefined` omission,
//! provider-metadata string lookup, tool-choice matching and stream-event decode errors.
//! Scope: pure helpers, crate-private.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/anthropic/body.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/anthropic/stream.rs
//! |-- provider-adapters/src/protocols/bedrock/body.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/protocols/bedrock/stream.rs
//! |-- provider-adapters/src/protocols/gemini/body.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! `-- provider-adapters/src/protocols/gemini/stream.rs
//! Invariants: object keys keep insertion order (the TS object-literal order); `None` values
//! are omitted exactly like `undefined` in `JSON.stringify`.
//! Contract tests: provider-adapters/tests/anthropic_messages.rs

use opencode_llm_api::{LlmError, ProviderMetadata, ToolChoice, ToolChoiceType};
use serde_json::{Map, Value};

/// JS number on the wire: integral values without a fractional part.
pub(crate) fn num(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Ordered JSON object builder that drops `None` (JS `undefined`).
#[derive(Default)]
pub(crate) struct Obj(Map<String, Value>);

impl Obj {
    pub(crate) fn new() -> Self {
        Self(Map::new())
    }

    pub(crate) fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn opt(mut self, key: &str, value: Option<impl Into<Value>>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.to_owned(), value.into());
        }
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn build(self) -> Value {
        Value::Object(self.0)
    }
}

/// `providerMetadata[provider][key]` when it is a string.
pub(crate) fn meta_str(
    meta: Option<&ProviderMetadata>,
    provider: &str,
    key: &str,
) -> Option<String> {
    meta?.get(provider)?.get(key)?.as_str().map(str::to_owned)
}

/// `{ provider: { key: value } }`.
pub(crate) fn meta(provider: &str, key: &str, value: impl Into<Value>) -> ProviderMetadata {
    let mut inner = Map::new();
    inner.insert(key.to_owned(), value.into());
    ProviderMetadata::from([(provider.to_owned(), inner)])
}

/// `ProviderShared.matchToolChoice` outcome.
pub(crate) enum Choice<'a> {
    Auto,
    None,
    Required,
    Tool(&'a str),
}

/// `ProviderShared.matchToolChoice`: a `tool` choice without a name is a request error.
pub(crate) fn match_tool_choice<'a>(
    route: &str,
    choice: &'a ToolChoice,
) -> Result<Choice<'a>, LlmError> {
    Ok(match choice.kind {
        ToolChoiceType::Auto => Choice::Auto,
        ToolChoiceType::None => Choice::None,
        ToolChoiceType::Required => Choice::Required,
        ToolChoiceType::Tool => match choice.name.as_deref().filter(|n| !n.is_empty()) {
            Some(name) => Choice::Tool(name),
            None => {
                return Err(LlmError::invalid_request(format!(
                    "{route} tool choice requires a tool name"
                )));
            }
        },
    })
}

/// Route id from a `"{provider}/{route}"` label (the route that owns decode errors).
pub(crate) fn route_id(label: &str) -> &str {
    label.rsplit_once('/').map_or(label, |(_, id)| id)
}

/// `Invalid {provider}/{route} stream event` (route/client.ts `decodeEvent`).
pub(crate) fn invalid_event(label: &str, raw: String) -> LlmError {
    LlmError::invalid_output(
        route_id(label),
        format!("Invalid {label} stream event"),
        Some(raw),
    )
}

/// Decode a frame into a typed stream event with the baseline error shape.
pub(crate) fn decode_event<T: serde::de::DeserializeOwned>(
    label: &str,
    frame: crate::protocol::Frame,
) -> Result<T, LlmError> {
    match frame {
        crate::protocol::Frame::Text(text) => {
            serde_json::from_str(&text).map_err(|_| invalid_event(label, text))
        }
        crate::protocol::Frame::Json(value) => serde_json::from_value(value.clone())
            .map_err(|_| invalid_event(label, value.to_string())),
    }
}
