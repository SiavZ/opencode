//! Purpose: global lifecycle publication and durable synchronization HTTP bridge.
//! Scope: real event store and process database, with explicit replay ownership.
//! Uses:
//! rust/crates/
//! |-- event-api/src/store.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: replay ownership is not a Session execution lease; no synthetic history.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, RouteContext, Value, internal, json, required_payload};
use opencode_event_api::{
    Definition, EventStore, LocationRef, PublishOptions, ReplayOptions, SerializedEvent,
};
use opencode_storage_api::DatabaseExt;

impl GlobalServices {
    pub(super) async fn publish(
        &self,
        event: &'static str,
        directory: &str,
        data: Value,
    ) -> Result<(), LegacyError> {
        let events = self
            .events
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Event store not composed".into()))?;
        events
            .publish(
                &Definition::ephemeral(event),
                data,
                PublishOptions {
                    location: Some(LocationRef {
                        directory: directory.into(),
                        workspace_id: None,
                    }),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub(super) async fn sync(
        &self,
        operation: &str,
        context: &RouteContext,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let payload = required_payload(payload)?;
        match operation {
            "sync.replay" => {
                let events = self.events.as_ref().ok_or_else(||LegacyError::Unavailable("Event store not composed".into()))?;
                let rows: Vec<SerializedEvent> = serde_json::from_value(payload.get("events").cloned().unwrap_or(Value::Null)).map_err(|error|LegacyError::BadRequest(error.to_string()))?;
                let source = rows.first().ok_or_else(||LegacyError::BadRequest("At least one event is required".into()))?.aggregate_id.clone();
                events.replay_all(rows,ReplayOptions { publish:true,owner_id:context.workspace_id.clone(),strict_owner:true }).await.map_err(internal)?;
                Ok(json!({"sessionID":source}))
            }
            "sync.steal" => {
                let workspace = context.workspace_id.clone().ok_or_else(||LegacyError::BadRequest("Workspace ID is required".into()))?;
                let session = payload.get("sessionID").and_then(Value::as_str).ok_or_else(||LegacyError::BadRequest("Session ID is required".into()))?.to_owned();
                let value = json!({"sessionID":session});
                let events = self.events.as_ref().ok_or_else(||LegacyError::Unavailable("Event store not composed".into()))?;
                let store = opencode_session_runtime::legacy::store::LegacyStore::new(std::sync::Arc::new(opencode_session_runtime::projection::LocalSessionEvents::new(self.database.clone(),events.clone())));
                store.patch(&session, serde_json::Map::from_iter([("workspaceID".into(),json!(workspace))])).await.map_err(internal)?;
                Ok(value)
            }
            "sync.history.list" => self.database.read(move |sql| {
                let rows = sql.query("SELECT id, aggregate_id, seq, type, data FROM event ORDER BY seq ASC",&[])?;
                rows.iter().filter_map(|row| {
                    let aggregate = match row.get::<String>("aggregate_id") {Ok(value)=>value,Err(error)=>return Some(Err(error))};
                    let seq = match row.get::<i64>("seq") {Ok(value)=>value,Err(error)=>return Some(Err(error))};
                    if payload.get(&aggregate).and_then(Value::as_i64).is_some_and(|excluded|seq <= excluded) {return None;}
                    Some((||Ok(json!({"id":row.get::<String>("id")?,"aggregate_id":aggregate,"seq":seq,"type":row.get::<String>("type")?,"data":row.get_json::<Value>("data")?})))())
                }).collect::<Result<Vec<_>,opencode_storage_api::StorageError>>()
            }).await.map(Value::Array).map_err(internal),
            _ => Err(LegacyError::Unavailable(format!("Unknown synchronization operation: {operation}"))),
        }
    }
}
