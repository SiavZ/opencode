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
//! `-- none, reserved contract (owner: phase-07, consumers 04/05/06 added as crates land)
//! Invariants: portable (native + wasm), depends only on allowlisted first-party crates.
//! Contract tests: integration-api/src/mcp.rs, integration-api/src/plugin.rs

pub mod error;
pub mod language;
pub mod mcp;
pub mod plugin;
pub mod plugin_abi;

pub use error::IntegrationError;
