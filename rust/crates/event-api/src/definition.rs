//! Purpose: event definitions, payloads and serialized durable rows (`packages/schema/src/event.ts`).
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! `-- event-api/src/store.rs
//! Invariants: stored type is `versioned_type(type, version)` = `"{type}.{version}"`. Payload JSON keys are
//! `id, metadata?, type, durable?, location?, data` with optional keys omitted, matching TS encoding.
//! Contract tests: event-api/src/definition.rs

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Durable settings of a definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableSpec {
    pub version: u32,
    /// Name of the string field in `data` that identifies the aggregate.
    pub aggregate: &'static str,
}

/// Encode/validate a data payload into its stored JSON form. Mirrors `Schema.encodeUnknownSync(definition.data)`.
pub type Codec = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

/// One event definition (type plus optional durable spec plus data codec).
#[derive(Clone)]
pub struct Definition {
    pub event_type: &'static str,
    pub durable: Option<DurableSpec>,
    /// Encode data for storage. `None` stores the data unchanged.
    pub encode: Option<Codec>,
    /// Decode stored data into the in-memory form. `None` returns the stored data unchanged.
    pub decode: Option<Codec>,
}

impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Definition")
            .field("event_type", &self.event_type)
            .field("durable", &self.durable)
            .finish_non_exhaustive()
    }
}

impl Definition {
    pub const fn ephemeral(event_type: &'static str) -> Self {
        Self {
            event_type,
            durable: None,
            encode: None,
            decode: None,
        }
    }

    pub const fn durable(event_type: &'static str, version: u32, aggregate: &'static str) -> Self {
        Self {
            event_type,
            durable: Some(DurableSpec { version, aggregate }),
            encode: None,
            decode: None,
        }
    }

    pub fn with_codec(mut self, encode: Codec, decode: Codec) -> Self {
        self.encode = Some(encode);
        self.decode = Some(decode);
        self
    }

    /// Stored `event.type` column, or `None` for ephemeral definitions.
    pub fn stored_type(&self) -> Option<String> {
        self.durable
            .map(|durable| versioned_type(self.event_type, durable.version))
    }
}

/// `Event.versionedType` from `packages/schema/src/event.ts`.
pub fn versioned_type(event_type: &str, version: u32) -> String {
    format!("{event_type}.{version}")
}

/// `Location.Ref` (`packages/schema/src/location.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocationRef {
    pub directory: String,
    #[serde(
        rename = "workspaceID",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub workspace_id: Option<String>,
}

/// Durable coordinates with aggregate ID, wire shape `{ aggregateID, seq, version }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Durable {
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub seq: i64,
    pub version: u32,
}

/// In-memory event payload (`Event.Payload`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub durable: Option<Durable>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub location: Option<LocationRef>,
    pub data: Value,
}

/// Serialized durable row used by replay/sync (`EventV2.SerializedEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializedEvent {
    pub id: String,
    /// Versioned stored type, e.g. `session.next.prompted.1`.
    #[serde(rename = "type")]
    pub event_type: String,
    pub seq: i64,
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub data: Value,
}

/// Prefix of event identifiers (`evt_` + ascending ID body).
pub const EVENT_ID_PREFIX: &str = "evt_";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_omits_optional_keys_in_ts_order() {
        let payload = Payload {
            id: "evt_1".into(),
            metadata: None,
            event_type: "session.next.prompted".into(),
            durable: Some(Durable {
                aggregate_id: "ses_1".into(),
                seq: 0,
                version: 1,
            }),
            location: Some(LocationRef {
                directory: "/tmp".into(),
                workspace_id: None,
            }),
            data: serde_json::json!({"sessionID": "ses_1"}),
        };
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"id":"evt_1","type":"session.next.prompted","durable":{"aggregateID":"ses_1","seq":0,"version":1},"location":{"directory":"/tmp"},"data":{"sessionID":"ses_1"}}"#
        );
        assert_eq!(versioned_type("a.b", 2), "a.b.2");
        assert_eq!(
            Definition::durable("x", 1, "sessionID")
                .stored_type()
                .as_deref(),
            Some("x.1")
        );
    }
}
