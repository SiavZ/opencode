//! Purpose: supply workspace lifecycle with the production credential and Session services.
//! Scope: auth forwarding, explicit OTEL environment and joined Session removal.
//! Uses:
//! rust/crates/
//! |-- locations/src/workspace/host.rs
//! |-- runtime/src/legacy.rs
//! |-- runtime/src/provider_services.rs
//! `-- session-api/src/operations.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: auth is read from canonical storage; no ambient environment or independent execution owner.
//! Contract tests: runtime/tests/workspace_transport.rs

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use opencode_locations::LocationError;
use opencode_locations::workspace::WorkspaceHost;
use opencode_session_api::operations::SessionService;
use opencode_session_api::{SessionError, SessionID};

pub(crate) struct Host {
    pub providers: Arc<crate::provider_services::ProviderServices>,
    pub session: Arc<dyn SessionService>,
    pub legacy: Arc<crate::legacy::LegacyAdapter>,
    pub environment: BTreeMap<String, String>,
}

#[async_trait]
impl WorkspaceHost for Host {
    async fn auth_content(&self) -> Result<String, LocationError> {
        let credentials = self
            .providers
            .auth_list()
            .await
            .map_err(LocationError::Io)?;
        serde_json::to_string(&credentials).map_err(|error| LocationError::Io(error.to_string()))
    }

    async fn remove_session(&self, session_id: &str) -> Result<(), LocationError> {
        let id = SessionID::new(session_id.to_owned())
            .map_err(|error| LocationError::Validation(error.to_string()))?;
        self.legacy
            .cancel_tree(session_id)
            .await
            .map_err(|error| match error {
                opencode_server::services::legacy::LegacyError::NotFound(message) => {
                    LocationError::NotFound(message)
                }
                other => LocationError::Io(other.to_string()),
            })?;
        self.session.delete(&id).await.map_err(|error| match error {
            SessionError::NotFound { .. } => LocationError::NotFound(error.to_string()),
            SessionError::Cancelled => LocationError::Cancelled,
            other => LocationError::Io(other.to_string()),
        })
    }

    fn otel_env(&self) -> Vec<(String, Option<String>)> {
        [
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_RESOURCE_ATTRIBUTES",
        ]
        .into_iter()
        .map(|key| (key.to_owned(), self.environment.get(key).cloned()))
        .collect()
    }
}
