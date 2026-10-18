//! Purpose: crate root of the portable OpenCode network client. Exposes typed current and
//! legacy operations, the transport boundary and failure separation. Never composes servers.
//! Scope: network-only client surface, native (reqwest) and wasm32 (browser fetch) targets.
//! Uses:
//! rust/crates/
//! |-- client/src/errors.rs
//! `-- client/src/transport/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-08, consumers 09/10/11/14 and sdk-embedded)
//! Invariants: never depends on server, runtime, session-runtime, storage-sqlite or
//! platform-native (contracts/crate-edges.toml), compiles for wasm32-unknown-unknown.
//! Contract tests: client/tests (Phase 08)

pub mod errors;
pub mod transport;

pub use errors::{ClientError, ClientErrorReason};
