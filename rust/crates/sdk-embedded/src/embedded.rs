//! Purpose: opt-in in-process composition facade, the Rust shape of
//! packages/sdk-next/src/opencode.ts:10-43: one shared service graph feeding both the embedded
//! no-auth router and local tool registration, a client bound to that router, joined disposal.
//! Scope: one embedded host per `OpenCode` value.
//! Uses:
//! `-- none
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- sdk-embedded/src/lib.rs
//! `-- sdk-embedded/tests/embedded.rs
//! Invariants: never fabricates a router or services. Until opencode-server exposes an
//! embeddable router and opencode-runtime exposes the application tool/permission graph,
//! `OpenCode::create` fails with `EmbedError::NotYetPorted` naming the first missing component.
//! Contract tests: sdk-embedded/tests/embedded.rs

use thiserror::Error;

/// Base URL the embedded client addresses (opencode.ts:34). Requests never leave the process.
pub const EMBEDDED_BASE_URL: &str = "http://opencode.local";

/// Basic-auth username of the embedded router; embedding always runs with no password, so
/// authentication is disabled (packages/server/src/routes.ts:47-48).
pub const EMBEDDED_AUTH_USERNAME: &str = "opencode";

/// Components the embedded composition needs, in the order `create` acquires them.
pub const REQUIRED_COMPONENTS: [Component; 3] = [
    Component::RuntimeServices,
    Component::EmbeddedRouter,
    Component::InProcessClientTransport,
];

/// One piece of the composition in opencode.ts:10-43.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// `AppNodeBuilder.build([ApplicationTools.node, PermissionSaved.node])` with a shared memo
    /// map (opencode.ts:11-19). Owner: phase 91 (`opencode-runtime`).
    RuntimeServices,
    /// `createEmbeddedRoutes()` with no-auth config, request-scoped `PermissionSaved` and a
    /// disposable web handler (opencode.ts:20-31). Owner: phase 06 (`opencode-server`).
    EmbeddedRouter,
    /// Client transport that dispatches to the in-process handler instead of the network
    /// (opencode.ts:32-37). Owner: phase 08 (`opencode-client`).
    InProcessClientTransport,
}

impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeServices => {
                "opencode-runtime: application tools + saved permissions graph"
            }
            Self::EmbeddedRouter => {
                "opencode-server: embedded no-auth router with disposable handler"
            }
            Self::InProcessClientTransport => "opencode-client: in-process transport",
        }
    }
}

/// Embedded composition failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmbedError {
    /// A required crate does not expose the API yet. Not retryable within this build.
    #[error("embedded composition not yet ported: {}", component.as_str())]
    NotYetPorted { component: Component },
}

/// Embedded OpenCode host: client operations plus `tools.register` (opencode.ts:38-41).
///
/// Values can only be produced by [`OpenCode::create`], which currently always reports the
/// missing component instead of returning a partially wired host.
#[derive(Debug)]
pub struct OpenCode {
    _private: (),
}

impl OpenCode {
    /// Port of `OpenCode.create` (opencode.ts:10). Builds runtime services first, so the first
    /// missing component is reported.
    pub async fn create() -> Result<Self, EmbedError> {
        Err(EmbedError::NotYetPorted {
            component: REQUIRED_COMPONENTS[0],
        })
    }
}
