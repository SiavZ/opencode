//! Purpose: `event.ts` event envelope (`define`), definition metadata, and the `inventory`,
//! `latest`, `durable` and `versionedType` registry helpers.
//! Scope: pure values and registry functions.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/location.rs
//! `-- schema/src/values.rs
//! External:
//! |-- indexmap::IndexMap
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: envelope key order is `id, metadata, type, durable, location, data` (the
//! baseline Struct order); optional keys are omitted; `type` must equal the definition type.
//! `latest` keeps the highest durable version and errors on duplicate non-versioned types;
//! `durable` keys are `<type>.<version>` and errors on duplicates.
//! Contract tests: schema/src/event.rs (unit tests), schema/tests/baseline_fixtures.rs

use crate::ids::EventID;
use crate::location;
use crate::values::{Int, JsonObject, optional};
use indexmap::IndexMap;
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Durability declaration of a definition (`define({ durable })`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DurableSpec {
    pub version: u32,
    pub aggregate: &'static str,
}

/// Static description of one event definition, the Rust equivalent of a `Definition` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Definition {
    pub r#type: &'static str,
    pub durable: Option<DurableSpec>,
}

/// Implemented by every event payload struct (`define({ schema })` fields).
pub trait EventData: Serialize + DeserializeOwned {
    const DEFINITION: Definition;
}

/// Envelope `durable` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Durable {
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub seq: Int,
    pub version: Int,
}

/// Encoded event: `{ id, metadata?, type, durable?, location?, data }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Event<D> {
    pub id: EventID,
    pub metadata: Option<JsonObject>,
    pub durable: Option<Durable>,
    pub location: Option<location::Ref>,
    pub data: D,
}

impl<D: EventData> Event<D> {
    /// New envelope with a fresh `evt_` ID and no optional parts.
    pub fn new(data: D) -> Self {
        Self {
            id: EventID::create(),
            metadata: None,
            durable: None,
            location: None,
            data,
        }
    }
    pub const fn r#type(&self) -> &'static str {
        D::DEFINITION.r#type
    }
}

#[derive(Serialize)]
struct EnvelopeOut<'a, D> {
    id: &'a EventID,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: &'a Option<JsonObject>,
    r#type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable: &'a Option<Durable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: &'a Option<location::Ref>,
    data: &'a D,
}

#[derive(Deserialize)]
struct EnvelopeIn<D> {
    id: EventID,
    #[serde(default, deserialize_with = "optional::deserialize")]
    metadata: Option<JsonObject>,
    r#type: String,
    #[serde(default, deserialize_with = "optional::deserialize")]
    durable: Option<Durable>,
    #[serde(default, deserialize_with = "optional::deserialize")]
    location: Option<location::Ref>,
    data: D,
}

impl<D: EventData> Serialize for Event<D> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        EnvelopeOut {
            id: &self.id,
            metadata: &self.metadata,
            r#type: D::DEFINITION.r#type,
            durable: &self.durable,
            location: &self.location,
            data: &self.data,
        }
        .serialize(serializer)
    }
}

impl<'de, D: EventData> Deserialize<'de> for Event<D> {
    fn deserialize<De: Deserializer<'de>>(deserializer: De) -> Result<Self, De::Error> {
        let input = EnvelopeIn::<D>::deserialize(deserializer)?;
        if input.r#type != D::DEFINITION.r#type {
            return Err(de::Error::custom(format!(
                "Expected \"{}\", got \"{}\"",
                D::DEFINITION.r#type,
                input.r#type
            )));
        }
        Ok(Self {
            id: input.id,
            metadata: input.metadata,
            durable: input.durable,
            location: input.location,
            data: input.data,
        })
    }
}

/// Registry errors thrown by `latest` and `durable` in the baseline.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("Duplicate latest event definition for {0}")]
    DuplicateLatest(String),
    #[error("Duplicate durable event definition for {0}")]
    DuplicateDurable(String),
}

/// `versionedType(type, version)`.
pub fn versioned_type(r#type: &str, version: u32) -> String {
    format!("{type}.{version}")
}

/// `latest(definitions)`: insertion-ordered map by type keeping the highest durable version.
/// Identical definitions listed twice are allowed (the baseline compares by identity).
pub fn latest(
    definitions: &[Definition],
) -> Result<IndexMap<&'static str, Definition>, RegistryError> {
    definitions
        .iter()
        .try_fold(IndexMap::new(), |mut result, definition| {
            let Some(existing) = result.get(definition.r#type).copied() else {
                result.insert(definition.r#type, *definition);
                return Ok(result);
            };
            if let (Some(next), Some(current)) = (definition.durable, existing.durable)
                && next.version != current.version
            {
                if next.version > current.version {
                    result.insert(definition.r#type, *definition);
                }
                return Ok(result);
            }
            if *definition != existing {
                return Err(RegistryError::DuplicateLatest(
                    definition.r#type.to_string(),
                ));
            }
            Ok(result)
        })
}

/// `durable(definitions)`: map keyed by `<type>.<version>` of durable definitions only.
pub fn durable(definitions: &[Definition]) -> Result<IndexMap<String, Definition>, RegistryError> {
    definitions
        .iter()
        .try_fold(IndexMap::new(), |mut result, definition| {
            let Some(spec) = definition.durable else {
                return Ok(result);
            };
            let key = versioned_type(definition.r#type, spec.version);
            if result.contains_key(&key) {
                return Err(RegistryError::DuplicateDurable(key));
            }
            result.insert(key, *definition);
            Ok(result)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Replied {
        #[serde(rename = "sessionID")]
        session_id: String,
        reply: String,
    }

    impl EventData for Replied {
        const DEFINITION: Definition = Definition {
            r#type: "test.replied",
            durable: None,
        };
    }

    const V1: Definition = Definition {
        r#type: "test.versioned",
        durable: Some(DurableSpec {
            version: 1,
            aggregate: "id",
        }),
    };
    const V2: Definition = Definition {
        r#type: "test.versioned",
        durable: Some(DurableSpec {
            version: 2,
            aggregate: "id",
        }),
    };

    #[test]
    fn envelope_key_order_and_omission() {
        let json = r#"{"data":{"reply":"once","sessionID":"ses_1"},"type":"test.replied","metadata":{},"id":"evt_1"}"#;
        let event: Event<Replied> = serde_json::from_str(json).unwrap();
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"id":"evt_1","metadata":{},"type":"test.replied","data":{"sessionID":"ses_1","reply":"once"}}"#
        );
    }

    #[test]
    fn envelope_rejects_wrong_type_and_null_optional() {
        let wrong = r#"{"id":"evt_1","type":"other","data":{"sessionID":"s","reply":"r"}}"#;
        assert!(serde_json::from_str::<Event<Replied>>(wrong).is_err());
        let null = r#"{"id":"evt_1","type":"test.replied","location":null,"data":{"sessionID":"s","reply":"r"}}"#;
        assert!(serde_json::from_str::<Event<Replied>>(null).is_err());
    }

    #[test]
    fn latest_is_order_independent() {
        assert_eq!(latest(&[V1, V2]).unwrap()["test.versioned"], V2);
        assert_eq!(latest(&[V2, V1]).unwrap()["test.versioned"], V2);
        let plain = Definition {
            r#type: "test.versioned",
            durable: None,
        };
        assert!(latest(&[plain, V1]).is_err());
    }

    #[test]
    fn durable_indexes_by_versioned_type() {
        let map = durable(&[V1, V2]).unwrap();
        assert!(map.contains_key("test.versioned.1") && map.contains_key("test.versioned.2"));
        assert_eq!(
            durable(&[V1, V1]).unwrap_err().to_string(),
            "Duplicate durable event definition for test.versioned.1"
        );
    }
}
