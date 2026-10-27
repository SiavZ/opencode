//! Purpose: prove Location event shutdown cancels a held real plugin event future.
//! Scope: SQLite event store and explicit test plugin suspension.
//! Uses:
//! rust/crates/
//! |-- event-store/src/lib.rs
//! `-- runtime/src/location_integrations/events.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::sync::Notify
//! Used by:
//! `-- none, entrypoint
//! Invariants: shutdown joins without requiring an uncooperative hook to return.
//! Contract tests: this file

use opencode_event_api::{Definition, EventStore, LocationRef, PublishOptions};
use opencode_event_store::SqlEventStore;
use opencode_integration_api::{
    IntegrationError,
    plugin::{HookCapabilities, PluginId, PluginInstance, ToolContext, ToolResult, TriggerName},
};
use opencode_runtime::location_integrations::events::IntegrationEvents;
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

struct Held {
    id: PluginId,
    capabilities: HookCapabilities,
    entered: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl PluginInstance for Held {
    fn id(&self) -> &PluginId {
        &self.id
    }
    fn capabilities(&self) -> &HookCapabilities {
        &self.capabilities
    }
    async fn event(&self, _: &Value) -> Result<(), IntegrationError> {
        self.entered.notify_one();
        std::future::pending().await
    }
    async fn trigger(
        &self,
        _: TriggerName,
        _: &Value,
        output: Value,
    ) -> Result<Value, IntegrationError> {
        Ok(output)
    }
    async fn config(&self, _: &Value) -> Result<(), IntegrationError> {
        Ok(())
    }
    async fn execute_tool(
        &self,
        _: &str,
        _: Value,
        _: ToolContext,
    ) -> Result<ToolResult, IntegrationError> {
        unreachable!("event fixture has no tool")
    }
    async fn dispose(&self) -> Result<(), IntegrationError> {
        Ok(())
    }
}

#[tokio::test]
async fn shutdown_cancels_held_plugin_event_and_joins_pump() {
    let database = Arc::new(SqliteDatabase::open_in_memory().await.unwrap());
    let store = Arc::new(SqlEventStore::new(database));
    let location = LocationRef {
        directory: "/isolated/fixture".into(),
        workspace_id: None,
    };
    let events = IntegrationEvents::start(store.clone(), location.clone());
    let plugin = Arc::new(Held {
        id: PluginId("held".into()),
        capabilities: HookCapabilities {
            event: true,
            ..Default::default()
        },
        entered: tokio::sync::Notify::new(),
    });
    events.set_plugins(vec![plugin.clone()]);
    store
        .publish(
            &Definition::ephemeral("fixture.event"),
            json!({"observed":true}),
            PublishOptions {
                location: Some(location),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), plugin.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), events.shutdown())
        .await
        .expect("held plugin must not retain Location shutdown");
    events.shutdown().await;
}
