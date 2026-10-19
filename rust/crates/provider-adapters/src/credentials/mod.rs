//! Purpose: Resolve stored API keys and OAuth credential material with environment fallback.
//! Scope: Explicitly supplied persisted credential records and registered environment names.
//! Uses:
//! rust/crates/
//! `-- schema/src/ids.rs
//! External:
//! |-- fmt::Debug
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- indexmap::IndexMap
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/src/lib.rs
//! |-- provider-adapters/tests/catalog.rs
//! |-- provider-adapters/tests/credentials_store.rs
//! `-- runtime/src/location_models.rs
//! Invariants: stored records precede environment credentials; secrets are never included in Debug output.
//! Contract tests: provider-adapters/tests/credentials_store.rs

use indexmap::IndexMap;
use opencode_schema::ids::IntegrationID;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CredentialValue {
    #[serde(rename = "key")]
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Value>>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(rename = "methodID")]
        method_id: String,
        refresh: String,
        access: String,
        expires: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Value>>,
    },
}
impl fmt::Debug for CredentialValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key { .. } => f.write_str("Key(<redacted>)"),
            Self::OAuth { .. } => f.write_str("OAuth(<redacted>)"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("invalid persisted credential: {0}")]
    Invalid(String),
}

#[derive(Clone, Default)]
pub struct CredentialStore {
    stored: IndexMap<IntegrationID, CredentialValue>,
    environment: IndexMap<IntegrationID, Vec<String>>,
    snapshot: IndexMap<String, String>,
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialStore")
            .field("stored", &self.stored)
            .field("environment", &self.environment)
            .field("snapshot", &"<redacted>")
            .finish()
    }
}

impl CredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn from_config(config: &Value) -> Self {
        let mut store = Self::new();
        store.apply_config(config);
        store
    }
    pub fn apply_config(&mut self, config: &Value) {
        if let Some(providers) = config.get("providers").and_then(Value::as_object) {
            for (id, entry) in providers {
                if let Some(names) = entry.get("env").and_then(Value::as_array) {
                    self.register_env(
                        IntegrationID::new(
                            entry
                                .get("integrationID")
                                .and_then(Value::as_str)
                                .unwrap_or(id),
                        ),
                        names
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    );
                }
            }
        }
    }
    pub fn with_environment(mut self, environment: IndexMap<String, String>) -> Self {
        self.snapshot = environment;
        self
    }
    pub fn load_cache(&mut self, path: &std::path::Path) -> Result<(), CredentialError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(CredentialError::Invalid("cannot read model cache".into())),
        };
        let input: Value = serde_json::from_slice(&bytes)
            .map_err(|_| CredentialError::Invalid("invalid model cache".into()))?;
        let providers = input
            .as_object()
            .ok_or_else(|| CredentialError::Invalid("expected provider object".into()))?;
        for (id, provider) in providers {
            if let Some(names) = provider
                .get("env")
                .and_then(Value::as_array)
                .filter(|names| !names.is_empty())
            {
                self.register_env(
                    IntegrationID::new(id),
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                );
            }
        }
        Ok(())
    }
    pub fn has_integration(&self, id: &IntegrationID) -> bool {
        self.environment.contains_key(id) || self.stored.contains_key(id)
    }
    pub fn register_env(&mut self, id: IntegrationID, names: Vec<String>) {
        self.environment.insert(id, names);
    }
    pub fn insert(&mut self, id: IntegrationID, value: CredentialValue) {
        self.stored.insert(id, value);
    }
    pub fn load_records(&mut self, input: &Value) -> Result<(), CredentialError> {
        let Some(records) = input.as_array() else {
            return Err(CredentialError::Invalid(
                "expected credential records array".into(),
            ));
        };
        let mut decoded = IndexMap::new();
        for record in records {
            if record.get("integration_id").is_some_and(Value::is_null) {
                continue;
            }
            let id = record
                .get("integrationID")
                .or_else(|| record.get("integration_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| CredentialError::Invalid("missing integration ID".into()))?;
            let input = record
                .get("value")
                .cloned()
                .ok_or_else(|| CredentialError::Invalid("missing value".into()))?;
            let input = match input {
                Value::String(value) => serde_json::from_str(&value)
                    .map_err(|_| CredentialError::Invalid("invalid credential JSON".into()))?,
                value => value,
            };
            let value = serde_json::from_value(input)
                .map_err(|_| CredentialError::Invalid("invalid credential value".into()))?;
            decoded.entry(IntegrationID::new(id)).or_insert(value);
        }
        self.stored.extend(decoded);
        Ok(())
    }
    pub fn active_credential(
        &self,
        id: &IntegrationID,
    ) -> Result<Option<CredentialValue>, CredentialError> {
        if let Some(value) = self.stored.get(id) {
            return Ok(Some(value.clone()));
        }
        Ok(self
            .environment
            .get(id)
            .into_iter()
            .flatten()
            .find_map(|name| {
                self.snapshot
                    .get(name)
                    .filter(|value| !value.is_empty())
                    .map(|key| CredentialValue::Key {
                        key: key.clone(),
                        metadata: None,
                    })
            }))
    }
}
