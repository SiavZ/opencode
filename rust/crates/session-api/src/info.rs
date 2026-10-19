//! Purpose: re-export `SessionV2.Info` and `Revert.State` from schema for Session consumers.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/revert.rs
//! `-- schema/src/session.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/session.rs
//! |-- server/tests/support/mod.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! `-- session-runtime/tests/admission_projection.rs
//! Invariants: schema owns the wire shape.
//! Contract tests: session-api/tests/wire.rs

pub use opencode_schema::revert::{FileDiff, State as RevertState};
pub use opencode_schema::session::{
    Direction as ListDirection, Info as SessionInfo, ListAnchor, Time as SessionTime,
};
