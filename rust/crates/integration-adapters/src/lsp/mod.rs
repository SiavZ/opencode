//! Purpose: LSP client adapter (server inventory, JSON-RPC, document sync, diagnostics). Owner: p07.lsp.
//! Scope: module root and public re-exports for runtime composition.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/diagnostics.rs
//! |-- integration-adapters/src/lsp/inventory.rs
//! |-- integration-adapters/src/lsp/jsonrpc.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/resolve_extra.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/src/lsp/text.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/lib.rs
//! Invariants: see rust/contracts/phase-07/lsp.md.
//! Contract tests: integration-adapters/tests/lsp_*.rs

pub mod client;
pub mod diagnostics;
pub mod inventory;
pub mod jsonrpc;
pub mod launch;
pub mod requests;
pub mod resolve;
mod resolve_extra;
pub mod roots;
pub mod service;
pub mod text;

pub use client::{Client, Handle, Timeouts};
pub use inventory::{Launch, ServerDef, builtins, configured};
pub use launch::{LaunchEnv, Spec};
pub use roots::{RootContext, RootRule};
pub use service::{LspOptions, LspService};
