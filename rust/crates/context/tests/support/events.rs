//! Purpose: fixture-labeled durable event publication and registered projectors for context tests.
//! Scope: real SQLite transactions, not the production event-store implementation.
//! Uses:
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/tests/support/mod.rs
//! |-- event-api/src/lib.rs
//! |-- schema/src/identifier.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- context/tests/support/mod.rs
//! Invariants: projectors then commit hook precede event writes; all share one transaction and roll back on error.
//! Contract tests: context/tests/epoch.rs, context/tests/history.rs

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_event_api::{
    AggregatePage, Definition, Durable, EventError, EventStore, EventStream, Listener, Payload,
    Projector, PublishOptions, ReplayOptions, SerializedEvent, Unsubscribe,
};
use opencode_storage_api::{StorageError, Value};

use super::{TestDb, transact};

type Registered = Vec<(String, Projector)>;

/// Durable publish fixture, standing in for opencode-event-store.
pub struct TestEvents {
    pub db: Arc<TestDb>,
    projectors: Mutex<Registered>,
}

impl TestEvents {
    pub fn new(db: Arc<TestDb>) -> Self {
        Self {
            db,
            projectors: Mutex::new(Vec::new()),
        }
    }

    /// Fixture projection for testing raw history and atomic hook behavior, not Session's projector.
    pub fn project_system_updates(&self) {
        self.project(&opencode_context::epoch::CONTEXT_UPDATED, Arc::new(|sql, payload| {
            let seq = payload.durable.as_ref().unwrap().seq;
            let timestamp = payload.data["timestamp"].as_i64().unwrap();
            sql.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES (?, ?, 'system', ?, ?, ?, ?)",
                &[
                    Value::from(payload.data["messageID"].as_str().unwrap()),
                    Value::from(payload.data["sessionID"].as_str().unwrap()),
                    Value::Integer(seq), Value::Integer(timestamp), Value::Integer(timestamp),
                    Value::json(&serde_json::json!({"text": payload.data["text"]}))?,
                ],
            )?;
            Ok(())
        }));
    }
}

fn unsupported() -> EventError {
    EventError::invalid("fixture", "not supported by the context test fixture")
}

fn storage(error: StorageError) -> EventError {
    EventError::Storage {
        message: error.to_string(),
        retryable: error.is_retryable(),
        source: Some(Box::new(error)),
    }
}

#[async_trait]
impl EventStore for TestEvents {
    async fn publish(
        &self,
        definition: &Definition,
        data: serde_json::Value,
        options: PublishOptions,
    ) -> Result<Payload, EventError> {
        let spec = definition.durable.ok_or_else(unsupported)?;
        let aggregate = data[spec.aggregate]
            .as_str()
            .ok_or_else(unsupported)?
            .to_string();
        let id = options
            .id
            .unwrap_or_else(|| format!("evt_{}", opencode_schema::identifier::ascending()));
        let stored_type = definition.stored_type().unwrap();
        let projectors = self
            .projectors
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _)| kind == definition.event_type)
            .map(|(_, projector)| projector.clone())
            .collect::<Vec<_>>();
        let conn = self.db.conn.lock().unwrap();
        transact(&conn, |sql| {
            let run = || -> Result<Payload, EventError> {
                let seq = sql.query_row("SELECT seq FROM event_sequence WHERE aggregate_id = ?", &[Value::from(aggregate.as_str())])
                    .map_err(storage)?.map(|row| row.get::<i64>("seq").map(|seq| seq + 1)).transpose().map_err(storage)?.unwrap_or(0);
                let payload = Payload {
                    id, metadata: options.metadata, event_type: definition.event_type.into(),
                    durable: Some(Durable { aggregate_id: aggregate.clone(), seq, version: spec.version }),
                    location: options.location, data,
                };
                for projector in projectors {
                    projector(sql, &payload).map_err(|source| EventError::Projector { event_type: definition.event_type.into(), source })?;
                }
                if let Some(hook) = options.commit {
                    hook(sql, seq).map_err(|source| EventError::Projector { event_type: definition.event_type.into(), source })?;
                }
                sql.execute("INSERT INTO event_sequence (aggregate_id, seq) VALUES (?, ?) ON CONFLICT(aggregate_id) DO UPDATE SET seq = excluded.seq",
                    &[Value::from(aggregate.as_str()), Value::Integer(seq)]).map_err(storage)?;
                sql.execute("INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?, ?, ?, ?, ?)",
                    &[Value::from(payload.id.as_str()), Value::from(aggregate.as_str()), Value::Integer(seq), Value::from(stored_type.as_str()), Value::json(&payload.data).map_err(storage)?]).map_err(storage)?;
                Ok(payload)
            };
            let result = run();
            let commit = result.is_ok();
            (result, commit)
        }).map_err(storage)?
    }

    fn project(&self, definition: &Definition, projector: Projector) {
        self.projectors
            .lock()
            .unwrap()
            .push((definition.event_type.into(), projector));
    }
    fn subscribe(&self, _: &Definition) -> EventStream {
        panic!("fixture: subscribe unsupported")
    }
    fn all(&self) -> EventStream {
        panic!("fixture: all unsupported")
    }
    fn all_bounded(&self, _: usize) -> EventStream {
        panic!("fixture: all_bounded unsupported")
    }
    fn durable(&self, _: &str, _: Option<i64>) -> EventStream {
        panic!("fixture: durable unsupported")
    }
    async fn read_aggregate(
        &self,
        _: &str,
        _: Option<i64>,
        _: usize,
        _: &[String],
    ) -> Result<AggregatePage, EventError> {
        Err(unsupported())
    }
    async fn latest_sequence(&self, _: &str) -> Result<i64, EventError> {
        Err(unsupported())
    }
    fn listen(&self, _: Listener) -> Unsubscribe {
        Box::new(|| {})
    }
    async fn replay(&self, _: SerializedEvent, _: ReplayOptions) -> Result<(), EventError> {
        Err(unsupported())
    }
    async fn replay_all(
        &self,
        _: Vec<SerializedEvent>,
        _: ReplayOptions,
    ) -> Result<Option<String>, EventError> {
        Err(unsupported())
    }
    async fn remove(&self, _: &str) -> Result<(), EventError> {
        Err(unsupported())
    }
    async fn claim(&self, _: &str, _: &str) -> Result<(), EventError> {
        Err(unsupported())
    }
}
