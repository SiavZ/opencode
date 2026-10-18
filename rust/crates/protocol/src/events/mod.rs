//! Purpose: versioned event unions (current `V2Event` from ServerDefinitions, legacy `Event` from
//! EventManifest.Latest) and the public event manifest.
//! Scope: static IR construction and typed event envelopes.
//! Uses:
//! rust/crates/
//! `-- protocol/src/ir.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/lib.rs
//! Invariants: current envelope `{id, type, data, metadata?, durable?, location?}`, legacy
//! `{id, type, properties}`. `server.connected` is appended when absent.
//! Contract tests: protocol/tests/events.rs

use crate::ir::Definition;

/// Event definitions in manifest order. Owner: p06.events-openapi.
pub fn definitions() -> Vec<Definition> {
    Vec::new()
}
