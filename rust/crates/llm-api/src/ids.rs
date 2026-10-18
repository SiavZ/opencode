//! Purpose: identifier newtypes and closed literal sets shared by the LLM contract
//! (model/provider ids, roles, finish reasons, reasoning effort, provider metadata).
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/message.rs
//! `-- llm-api/src/route.rs
//! Invariants: literal spellings match packages/llm/src/schema/ids.ts exactly.
//! Contract tests: llm-api/tests/contract.rs

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `Record<providerName, Record<string, unknown>>` escape hatch carried on parts, events
/// and usage (`@opencode-ai/schema/llm` `ProviderMetadata`).
pub type ProviderMetadata = indexmap::IndexMap<String, Map<String, Value>>;

/// Free-form JSON object (`Record<string, unknown>`).
pub type JsonObject = Map<String, Value>;

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

string_id!(
    /// Branded `LLM.ModelID`: the provider-native model id sent on the wire.
    ModelId
);
string_id!(
    /// Branded `LLM.ProviderID`.
    ProviderId
);

/// `ReasoningEfforts` literal set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextVerbosity {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Terminal reason of one provider step or the whole stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
    Unknown,
}

/// Serialize `Option<f64>` the way `JSON.stringify` prints JS numbers: integral values
/// without a fractional part (`1` not `1.0`), non-finite values as `null`.
pub mod js_number {
    use serde::{Deserialize, Deserializer, Serializer};

    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

    pub fn serialize<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            None => serializer.serialize_none(),
            Some(v) if !v.is_finite() => serializer.serialize_none(),
            Some(v) if v.fract() == 0.0 && v.abs() <= MAX_SAFE => {
                serializer.serialize_i64(*v as i64)
            }
            Some(v) => serializer.serialize_f64(*v),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<f64>, D::Error> {
        Option::<f64>::deserialize(deserializer)
    }
}
