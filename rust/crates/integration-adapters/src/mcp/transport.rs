//! Purpose: transport abstraction shared by stdio, streamable HTTP and legacy SSE MCP transports.
//! Scope: trait, inbound message envelope and transport error values. No I/O.
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-adapters/src/mcp/stdio.rs
//! `-- integration-adapters/src/mcp/streamable.rs
//! Invariants: a transport delivers inbound JSON-RPC messages on its bounded channel and sends
//! exactly one `Inbound::Closed` (or drops the sender) when the peer goes away.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use async_trait::async_trait;
use serde_json::Value;

/// Capacity of every inbound transport channel. Senders await (backpressure), never drop.
pub const INBOUND_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Message(Value),
    Closed(String),
}

/// Transport failure. `Unauthorized` mirrors the SDK `UnauthorizedError`, which drives the
/// needs_auth / needs_client_registration status transitions in the service.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Failed(String),
    #[error("Not connected")]
    Closed,
}

impl TransportError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized(_))
    }
}

/// OAuth discovery hints a remote transport learned from `WWW-Authenticate` headers. They are
/// reused by `finish_auth`, mirroring `transport.finishAuth` in the SDK.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthHints {
    pub resource_metadata_url: Option<String>,
    pub scope: Option<String>,
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, message: Value) -> Result<(), TransportError>;
    async fn close(&self);
    /// Child pid for stdio transports (descendant cleanup on shutdown).
    fn pid(&self) -> Option<u32> {
        None
    }
    /// HTTP transports send `mcp-protocol-version` after initialization.
    fn set_protocol_version(&self, _version: &str) {}
    fn auth_hints(&self) -> AuthHints {
        AuthHints::default()
    }
}
