//! Purpose: injected service bundle consumed by handlers and middleware, plus narrow port traits
//! for capabilities whose upstream crate trait does not exist yet. Owns no concrete adapters.
//! Scope: process-level dependency container, cloned cheaply per request.
//! Uses:
//! rust/crates/
//! |-- server/src/services/legacy.rs
//! |-- server/src/services/ports.rs
//! |-- server/src/services/streams.rs
//! `-- session-api/src/operations.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/process.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/lib.rs
//! |-- server/src/middleware/legacy_routing.rs
//! |-- server/src/middleware/location.rs
//! |-- server/src/services/streams.rs
//! |-- server/tests/http_errors.rs
//! |-- server/tests/http_legacy.rs
//! |-- server/tests/http_proxy.rs
//! |-- server/tests/replay_baseline.rs
//! |-- server/tests/sse_events.rs
//! |-- server/tests/support/mod.rs
//! `-- server/tests/ws_pty.rs
//! Invariants: an absent service is reported as a typed NotYetPorted error by handlers, never as
//! fake success. No Session-ID-scoped service lives here.
//! Contract tests: server/tests/http_session.rs

pub mod legacy;
pub mod ports;
pub mod streams;

use std::sync::Arc;

use opencode_session_api::operations::SessionService;

pub use ports::{
    LegacyWorkspaces, LocationInfo, LocationRef, LocationResolver, PortError, SessionPlacement,
    UiAssets, UiRequest, UiResponse, WorkspaceInfo, WorkspaceTarget,
};

/// Every service a handler may need. `None` means the capability is not composed in this host.
#[derive(Clone)]
pub struct Services {
    pub legacy_sessions: Option<Arc<dyn legacy::LegacySessions>>,
    pub legacy_global: Option<Arc<dyn legacy::LegacySessions>>,
    pub legacy_native: Option<Arc<dyn legacy::LegacySessions>>,
    pub legacy_integrations: Option<Arc<dyn legacy::LegacySessions>>,
    pub legacy_providers: Option<Arc<dyn legacy::LegacySessions>>,
    pub version: String,
    /// Phase 04 `SessionV2.Interface`.
    pub session: Option<Arc<dyn SessionService>>,
    /// Location resolution (`LocationServiceMap.get` + `Location.Info`).
    pub locations: Option<Arc<dyn LocationResolver>>,
    /// Persisted Session placement read by session-location middleware.
    pub placement: Option<Arc<dyn SessionPlacement>>,
    /// Legacy workspace routing (control-plane workspaces, adapters, sync state).
    pub workspaces: Option<Arc<dyn LegacyWorkspaces>>,
    /// Web UI assets for the full host fallback route.
    pub ui: Option<Arc<dyn UiAssets>>,
    /// Event source, legacy bus, PTY lookup, tickets and the WebSocket tracker.
    pub streams: streams::StreamServices,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            legacy_sessions: None,
            legacy_global: None,
            legacy_native: None,
            legacy_integrations: None,
            legacy_providers: None,
            version: "local".into(),
            session: None,
            locations: None,
            placement: None,
            workspaces: None,
            ui: None,
            streams: Default::default(),
        }
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("session", &self.session.is_some())
            .field("locations", &self.locations.is_some())
            .field("placement", &self.placement.is_some())
            .field("workspaces", &self.workspaces.is_some())
            .field("ui", &self.ui.is_some())
            .field("streams", &self.streams)
            .finish()
    }
}
