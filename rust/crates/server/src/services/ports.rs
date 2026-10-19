//! Purpose: narrow port traits the server needs from phases 02 (locations, workspaces) and 10
//! (web UI assets) until their crates publish equivalents. Data shapes follow the baseline wire.
//! Scope: object-safe async traits and wire values.
//! Uses:
//! rust/crates/
//! `-- schema/src/location.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- server/src/services/mod.rs
//! Invariants: `LocationInfo` is the schema `Location.Info`. Remote targets carry only the URL and extra headers to forward.
//! Contract tests: server/tests/http_session.rs, server/tests/http_proxy.rs

use async_trait::async_trait;

/// Infrastructure failure from a port. Maps to the defect path (500).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct PortError {
    pub message: String,
}

/// `Location.Info` (`packages/schema/src/location.ts`).
pub use opencode_schema::location::{Info as LocationInfo, Ref as LocationRef};

/// Resolve a location hint to the location the services run in (`LocationServiceMap.get`).
#[async_trait]
pub trait LocationResolver: Send + Sync {
    async fn resolve(&self, location: &LocationRef) -> Result<LocationInfo, PortError>;
}

/// Persisted Session placement (`SessionTable.directory`, `workspace_id`).
#[async_trait]
pub trait SessionPlacement: Send + Sync {
    /// `Ok(None)` when no Session row exists.
    async fn placement(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Option<String>)>, PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceTarget {
    Local {
        directory: String,
    },
    Remote {
        url: String,
        headers: Vec<(String, String)>,
    },
}

/// Legacy control-plane workspace routing (`Workspace.Service`, `WorkspaceAdapterRuntime`,
/// legacy `Session.Service.get` for placement).
#[async_trait]
pub trait LegacyWorkspaces: Send + Sync {
    /// `OPENCODE_WORKSPACE_ID` when this process runs inside a workspace.
    fn configured_workspace_id(&self) -> Option<String>;
    /// Legacy session `(directory, workspaceID)`, `None` when missing or unreadable.
    async fn session(&self, session_id: &str) -> Option<(String, Option<String>)>;
    async fn workspace(&self, workspace_id: &str) -> Option<WorkspaceInfo>;
    async fn target(&self, workspace: &WorkspaceInfo) -> WorkspaceTarget;
    async fn is_syncing(&self, workspace_id: &str) -> bool;
}

/// Static or proxied web UI (`packages/opencode/src/server/shared/ui.ts`).
pub type UiRequest = axum::extract::Request;
pub type UiResponse = axum::response::Response;

#[async_trait]
pub trait UiAssets: Send + Sync {
    /// Returns `(content_type, body, content_security_policy)` or `None` for 404.
    async fn get(&self, path: &str) -> Option<(String, Vec<u8>, Option<String>)>;

    /// Full proxy boundary. Native bodies own their streams, so dropping a response cancels it.
    /// Static adapters keep their existing `get` implementation without losing compatibility.
    async fn request(&self, request: UiRequest) -> Result<UiResponse, PortError> {
        use axum::http::{StatusCode, header};
        use axum::response::IntoResponse;

        let Some((content_type, body, csp)) = self.get(request.uri().path()).await else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        let mut response = (StatusCode::OK, body).into_response();
        if let Ok(value) = content_type.parse() {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        if let Some(value) = csp.and_then(|value| value.parse().ok()) {
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, value);
        }
        Ok(response)
    }
}
