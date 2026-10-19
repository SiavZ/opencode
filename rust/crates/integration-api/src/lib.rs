//! Purpose: crate root for Phase 07 integration contracts (MCP, language servers, plugin hooks).
//! Scope: pure values and async service traits. No process, network or SDK code.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/error.rs
//! |-- integration-api/src/language.rs
//! |-- integration-api/src/mcp.rs
//! |-- integration-api/src/plugin.rs
//! `-- integration-api/src/plugin_abi/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/jsonrpc.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/resolve_extra.rs
//! |-- integration-adapters/tests/lsp_client.rs
//! |-- integration-adapters/tests/lsp_inventory.rs
//! `-- integration-adapters/tests/lsp_jsonrpc.rs
//! Invariants: portable (native + wasm), depends only on allowlisted first-party crates.
//! Contract tests: integration-api/src/mcp.rs, integration-api/src/plugin.rs

pub mod error;
pub mod language;
pub mod mcp;
pub mod plugin;
pub mod plugin_abi;

pub use error::IntegrationError;
