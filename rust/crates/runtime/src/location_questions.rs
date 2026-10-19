//! Purpose: publish shared question domain events through the real Location event store.
//! Scope: composition adapter without a second pending registry.
//! Uses: event-api, session-runtime/projection, tools/interaction.
//! Used by: runtime/location and native_http.
//! Invariants: runner, HTTP, CLI and TUI receive the same Arc<Questions>.
//! Contract tests: runtime/tests/permissions.rs.

use async_trait::async_trait;
use opencode_event_api::{Definition, LocationRef, PublishOptions};
use opencode_session_runtime::projection::SessionEvents;
use opencode_tools::interaction::{QuestionEvents, Questions};
use std::sync::Arc;

pub fn build(events: Arc<dyn SessionEvents>, location: LocationRef) -> Arc<Questions> {
    Arc::new(Questions::new(Arc::new(Events { events, location })))
}

struct Events {
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
}

#[async_trait]
impl QuestionEvents for Events {
    async fn publish(&self, name: &'static str, value: serde_json::Value) -> Result<(), String> {
        self.events
            .store()
            .publish(
                &Definition::ephemeral(name),
                value,
                PublishOptions {
                    location: Some(self.location.clone()),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}
