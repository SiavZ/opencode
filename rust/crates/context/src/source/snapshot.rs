//! Purpose: namespaced source identities and durable snapshot/generation wire types.
//! Scope: pure serialization and validation, no source observation.
//! Uses:
//! `-- none
//! External:
//! |-- fmt::Display
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- indexmap::IndexMap
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::de::Error::custom
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- context/src/source/error.rs
//! `-- context/src/source/mod.rs
//! Invariants: snapshot keys are namespaced, removal text is absent or nonempty (never null), order is retained.
//! Contract tests: context/tests/system_context.rs

use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable namespaced key, pattern `^[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._/-]*$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Key(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid system context key: {0}")]
pub struct InvalidKey(pub String);

impl Key {
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidKey> {
        let value = value.into();
        if valid_key(&value) {
            return Ok(Self(value));
        }
        Err(InvalidKey(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_key(value: &str) -> bool {
    let Some((namespace, name)) = value.split_once('/') else {
        return false;
    };
    let head = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let starts = |s: &str| s.chars().next().is_some_and(head);
    starts(namespace)
        && starts(name)
        && namespace
            .chars()
            .all(|c| head(c) || matches!(c, '.' | '_' | '-'))
        && name
            .chars()
            .all(|c| head(c) || matches!(c, '.' | '_' | '-' | '/'))
}

impl TryFrom<String> for Key {
    type Error = InvalidKey;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Key> for String {
    fn from(key: Key) -> Self {
        key.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Durable comparison state for one admitted source. `removed` must be non-empty when present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub value: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_empty"
    )]
    pub removed: Option<String>,
}

fn non_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    // Missing is handled by serde's default. A present null is not an optional string.
    let value = String::deserialize(d)?;
    if value.is_empty() {
        return Err(serde::de::Error::custom(
            "removed must be a non-empty string",
        ));
    }
    Ok(Some(value))
}

/// Durable structured snapshot, insertion ordered like the baseline JSON object.
pub type Snapshot = IndexMap<Key, SourceSnapshot>;

/// Immutable baseline plus snapshot of one generation.
#[derive(Debug, Clone, PartialEq)]
pub struct Generation {
    pub baseline: String,
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReconcileResult {
    Unchanged,
    Updated { text: String, snapshot: Snapshot },
    ReplacementReady(Generation),
    ReplacementBlocked,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplacementResult {
    ReplacementReady(Generation),
    ReplacementBlocked,
}
