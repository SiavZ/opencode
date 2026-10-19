//! Purpose: workspace service inputs and the collaborators owned by other phases (auth content,
//! session removal, OTEL env) behind `WorkspaceHost`, with typed `Unsupported` defaults.
//! Scope: traits and plain values used by `WorkspaceService`.
//! Uses:
//! rust/crates/
//! `-- locations/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- locations/src/workspace/mod.rs
//! Invariants: defaults never silently succeed; `TIMEOUT` is the baseline 5000 ms.
//! Contract tests: locations/tests/workspace_lifecycle.rs

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::error::{LocationError, Result};

/// `TIMEOUT` of workspace.ts (create status wait, `waitForSync` default).
pub const TIMEOUT: Duration = Duration::from_millis(5000);

/// Collaborators owned by other phases. Defaults fail with `Unsupported`.
#[async_trait]
pub trait WorkspaceHost: Send + Sync {
    /// `JSON.stringify(yield* auth.all())` for `OPENCODE_AUTH_CONTENT`.
    async fn auth_content(&self) -> Result<String> {
        Err(LocationError::unsupported(
            "workspace.create auth",
            "Auth.all is owned by the provider auth phase",
        ))
    }

    /// `Session.remove(id)`. Return `NotFound` for missing sessions (ignored by `remove`).
    async fn remove_session(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        Err(LocationError::unsupported(
            "workspace.remove sessions",
            "Session.remove is owned by the session phase",
        ))
    }

    /// Extra process environment forwarded to new workspaces (`OTEL_*`).
    fn otel_env(&self) -> Vec<(String, Option<String>)> {
        [
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_RESOURCE_ATTRIBUTES",
        ]
        .into_iter()
        .map(|k| (k.to_owned(), std::env::var(k).ok()))
        .collect()
    }
}

/// Host with only the defaults (everything owned by later phases is `Unsupported`).
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedHost;
impl WorkspaceHost for UnsupportedHost {}

/// `Workspace.CreateInput`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateInput {
    pub id: Option<String>,
    pub kind: String,
    pub branch: Option<String>,
    pub project_id: String,
    pub extra: Option<Json>,
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct WorkspaceOptions {
    /// `RuntimeFlags.experimentalWorkspaces`.
    pub experimental_workspaces: bool,
    /// `OPENCODE_WORKSPACE_ID` of this process.
    pub configured_workspace_id: Option<String>,
    /// Status wait in `create` and default `waitForSync` timeout.
    pub timeout: Duration,
}

impl Default for WorkspaceOptions {
    fn default() -> Self {
        Self {
            experimental_workspaces: false,
            configured_workspace_id: None,
            timeout: TIMEOUT,
        }
    }
}
