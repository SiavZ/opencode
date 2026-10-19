//! Purpose: small wire helpers shared by the OpenAI protocols: Effect-Schema-compatible
//! optional field decoding, JS number encoding, `{ openai: {...} }` metadata, stream-event
//! decode errors and tool-choice lowering.
//! Scope: pure helpers.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! `-- provider-adapters/src/tool_schema.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::Serializer
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Map
//! |-- serde_json::Number::from_f64
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/protocols/openai/chat_stream.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! |-- provider-adapters/src/protocols/openai/responses_events.rs
//! `-- provider-adapters/src/protocols/openai/responses_stream.rs
//! Invariants: `present` mirrors `Schema.optional` (absent ok, explicit null rejected unless the
//! inner type accepts null), `nullable` mirrors `optionalNull` (absent vs null preserved).
//! Stream decode failures read `Invalid <provider>/<route> stream event` with the raw frame.
//! Contract tests: provider-adapters/tests/openai_chat.rs, provider-adapters/tests/openai_responses.rs

use opencode_llm_api::{LlmError, LlmRequest, ProviderMetadata, ToolChoice, ToolChoiceType};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::protocol::Frame;

/// `Schema.optional(T)`: absent -> `None`; present values must decode as `T`.
pub mod present {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<Option<T>, D::Error> {
        T::deserialize(deserializer).map(Some)
    }
}

/// `optionalNull(T)`: absent -> `None`, `null` -> `Some(None)`.
pub mod nullable {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// Flatten `Option<Option<T>>` (absent or null both read as missing).
pub fn flat<T>(value: &Option<Option<T>>) -> Option<&T> {
    value.as_ref().and_then(Option::as_ref)
}

/// JSON number as `JSON.stringify` prints a JS number.
pub fn js_num(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// `serialize_with` for `Option<f64>` echoed into provider metadata (JS number printing).
pub fn ser_js_num<S: serde::Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    value.map(js_num).serialize(s)
}

/// Insert `key` only when `value` is present (JS `undefined` fields vanish on encode).
pub fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    }
}

/// `{ openai: metadata }`.
pub fn openai_metadata(metadata: Map<String, Value>) -> ProviderMetadata {
    ProviderMetadata::from([("openai".to_owned(), metadata)])
}

/// Route id from the `"{provider}/{route}"` label passed to `ProtocolState::step`.
pub fn route_id(label: &str) -> &str {
    label.rsplit_once('/').map_or(label, |(_, id)| id)
}

/// `Protocol.jsonEvent(schema)` decode: parse and validate one frame.
pub fn decode_event<T: DeserializeOwned>(label: &str, frame: &Frame) -> Result<T, LlmError> {
    let fail = || {
        LlmError::invalid_output(
            route_id(label),
            format!("Invalid {label} stream event"),
            Some(frame.raw()),
        )
    };
    let value = match frame {
        Frame::Json(value) => value.clone(),
        Frame::Text(text) => serde_json::from_str(text).map_err(|_| fail())?,
    };
    serde_json::from_value(value).map_err(|_| fail())
}

/// `ProviderShared.matchToolChoice` with the provider-specific named-tool shape.
pub fn lower_tool_choice(
    route: &str,
    choice: &ToolChoice,
    named: impl FnOnce(&str) -> Value,
) -> Result<Value, LlmError> {
    Ok(match choice.kind {
        ToolChoiceType::Auto => Value::from("auto"),
        ToolChoiceType::None => Value::from("none"),
        ToolChoiceType::Required => Value::from("required"),
        ToolChoiceType::Tool => match choice.name.as_deref().filter(|n| !n.is_empty()) {
            Some(name) => named(name),
            None => {
                return Err(LlmError::invalid_request(format!(
                    "{route} tool choice requires a tool name"
                )));
            }
        },
    })
}

/// `ProviderShared.encodeJson`.
pub fn encode_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Tool parameters after model compatibility and the OpenAI projection.
pub fn tool_parameters(request: &LlmRequest, schema: &Map<String, Value>) -> Value {
    let compat = request
        .model
        .compatibility
        .as_ref()
        .and_then(|c| c.tool_schema);
    Value::Object(crate::tool_schema::openai(
        &crate::tool_schema::model_compatibility(schema, compat),
    ))
}
