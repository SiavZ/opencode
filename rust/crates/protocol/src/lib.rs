//! Purpose: crate root for the HTTP wire contract: operation registry, schema IR, error
//! envelopes, event unions and SSE framing. Does not perform I/O or own handlers.
//! Scope: pure, portable (native and wasm32) values and static registries.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/events/mod.rs
//! |-- protocol/src/ir.rs
//! |-- protocol/src/legacy/mod.rs
//! |-- protocol/src/openapi.rs
//! |-- protocol/src/operation.rs
//! `-- protocol/src/sse.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-06, consumers: client p08, view-model p10, server p06)
//! Invariants: depends only on opencode-schema, no tokio/axum, compiles for wasm32-unknown-unknown.
//! Contract tests: protocol/tests/registry.rs

pub mod current;
pub mod definitions;
pub mod errors;
pub mod events;
pub mod ir;
pub mod legacy;
pub mod openapi;
pub mod operation;
pub mod sse;

pub use operation::{Api, Endpoint, Group, Method};

/// Every registered API: the current Server API (`packages/protocol/src/api.ts`) followed by
/// the legacy PublicApi families (`packages/opencode/src/server/routes/instance/httpapi/api.ts`).
pub fn registry() -> &'static [&'static Api] {
    static ALL: std::sync::OnceLock<Vec<&'static Api>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| vec![current::api(), legacy::api()])
}
