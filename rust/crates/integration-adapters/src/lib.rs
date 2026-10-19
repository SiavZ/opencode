//! Purpose: crate root for native Phase 07 adapters implementing integration-api traits.
//! Scope: module declarations only. Each module is owned by one p07 child node.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/acp/mod.rs
//! |-- integration-adapters/src/lsp/mod.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/plugins/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: no dependency on server or UI crates.
//! Contract tests: integration-adapters/tests/

pub mod acp;
pub mod lsp;
pub mod mcp;
pub mod plugins;
