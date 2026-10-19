//! Purpose: MCP client adapter (stdio, streamable HTTP, legacy SSE, OAuth, catalog). Owner: p07.mcp.
//! Scope: module root and public re-exports; `McpServiceImpl` implements
//! `opencode_integration_api::mcp::McpService`.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! |-- integration-adapters/src/mcp/browser.rs
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! |-- integration-adapters/src/mcp/oauth.rs
//! |-- integration-adapters/src/mcp/oauth_callback.rs
//! |-- integration-adapters/src/mcp/oauth_discovery.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! |-- integration-adapters/src/mcp/oauth_token.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-adapters/src/mcp/service_api.rs
//! |-- integration-adapters/src/mcp/service_auth.rs
//! |-- integration-adapters/src/mcp/sse.rs
//! |-- integration-adapters/src/mcp/stdio.rs
//! |-- integration-adapters/src/mcp/streamable.rs
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lib.rs
//! |-- integration-adapters/tests/mcp_oauth.rs
//! `-- integration-adapters/tests/mcp_support/mod.rs
//! Invariants: see rust/contracts/phase-07/mcp.md.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs, integration-adapters/tests/mcp_http.rs,
//! integration-adapters/tests/mcp_oauth.rs

pub mod auth_store;
pub mod browser;
pub mod catalog;
pub mod client;
pub mod connect;
pub mod jsonrpc;
pub mod legacy_sse;
pub mod oauth;
pub mod oauth_callback;
pub mod oauth_discovery;
pub mod oauth_provider;
pub mod oauth_token;
pub mod remote;
pub mod service;
pub mod service_api;
pub mod service_auth;
pub mod sse;
pub mod stdio;
pub mod streamable;
pub mod transport;

pub use auth_store::McpAuthStore;
pub use browser::{BrowserOpener, SystemBrowser};
pub use oauth_callback::OAuthCallback;
pub use service::{McpEvents, McpOptions, McpServiceImpl, NoopEvents, Toast};
