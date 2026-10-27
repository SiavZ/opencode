//! Purpose: route plugin SDK callbacks through the fully composed production service graph.
//! Scope: deferred process initialization and weak Location callback handles.
//! Uses:
//! rust/crates/
//! |-- server/src/services.rs
//! `-- server/src/services/legacy.rs
//! External:
//! |-- async_trait::async_trait
//! `-- std::sync::OnceLock
//! Used by:
//! rust/crates/
//! |-- runtime/src/location.rs
//! `-- runtime/src/process.rs
//! Invariants: Location callbacks hold only a weak process-router reference, so the graph has no ownership cycle.
//! Contract tests: runtime/tests/location_integrations.rs

use async_trait::async_trait;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::Services;
use opencode_server::services::legacy::{
    LegacyBytesResponse, LegacyError, LegacyResponse, LegacySessions,
};
use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock, Weak};

#[derive(Default)]
pub(crate) struct ServiceRouter(OnceLock<Services>);

impl ServiceRouter {
    pub(crate) fn initialize(&self, services: Services) {
        assert!(
            self.0.set(services).is_ok(),
            "process service router initialized twice"
        );
    }

    pub(crate) fn client(self: &Arc<Self>) -> Arc<dyn LegacySessions> {
        Arc::new(CallbackClient(Arc::downgrade(self)))
    }
}

struct CallbackClient(Weak<ServiceRouter>);

pub(crate) fn domain<'a>(
    services: &'a Services,
    operation: &str,
) -> Option<&'a Arc<dyn LegacySessions>> {
    let current = operation.strip_prefix("v2.").unwrap_or(operation);
    match current {
        "config.providers" => services.legacy_providers.as_ref(),
        "app.skills" | "experimental.resource.list" => services.legacy_integrations.as_ref(),
        _ => match current.split('.').next().unwrap_or_default() {
            "session" => services.legacy_sessions.as_ref(),
            "provider" | "auth" | "model" | "integration" => services.legacy_providers.as_ref(),
            "permission" | "question" | "pty" | "tool" => services.legacy_native.as_ref(),
            "mcp" | "lsp" | "skill" | "reference" => services.legacy_integrations.as_ref(),
            _ => services.legacy_global.as_ref(),
        },
    }
}

#[async_trait]
impl LegacySessions for CallbackClient {
    async fn handle_bytes(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<LegacyBytesResponse, LegacyError> {
        let router = self
            .0
            .upgrade()
            .ok_or_else(|| LegacyError::Unavailable("process services have shut down".into()))?;
        let services = router
            .0
            .get()
            .ok_or_else(|| LegacyError::Unavailable("process services are initializing".into()))?;
        let service = domain(services, operation).ok_or_else(|| {
            LegacyError::Unavailable(format!("service unavailable for {operation}"))
        })?;
        service
            .handle_bytes(operation, context, params, query, payload)
            .await
    }

    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        Ok(self
            .handle_response(operation, context, params, query, payload)
            .await?
            .body)
    }

    async fn handle_response(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<LegacyResponse, LegacyError> {
        let router = self
            .0
            .upgrade()
            .ok_or_else(|| LegacyError::Unavailable("process services have shut down".into()))?;
        let services = router
            .0
            .get()
            .ok_or_else(|| LegacyError::Unavailable("process services are initializing".into()))?;
        let service = domain(services, operation).ok_or_else(|| {
            LegacyError::Unavailable(format!("service unavailable for {operation}"))
        })?;
        service
            .handle_response(operation, context, params, query, payload)
            .await
    }
}
