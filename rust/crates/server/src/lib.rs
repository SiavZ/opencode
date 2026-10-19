//! Purpose: crate root for the Rust OpenCode HTTP server: router assembly over injected service
//! traits, middleware, transports and listener lifecycle. Owns no domain policy.
//! Scope: process-level listener plus per-request/per-connection handlers.
//! Uses:
//! rust/crates/
//! |-- protocol/src/lib.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/middleware/mod.rs
//! |-- server/src/services/mod.rs
//! `-- server/src/transport/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: listener bound before readiness is reported, every spawned task is tracked and
//! joined on shutdown, no session-ID-scoped layers.
//! Contract tests: server/tests (added by p06 children)

/// Wire contract re-export so hosts depend on one version.
pub use opencode_protocol as protocol;

pub mod app;
pub mod handlers;
pub mod lifecycle;
pub mod middleware;
pub mod services;
pub mod transport;
