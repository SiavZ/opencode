//! Purpose: re-export `SessionV2.Info` and `Revert.State` from schema for Session consumers.
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! `-- opencode_schema::{revert, session}
//! Used by:
//! rust/crates/
//! |-- session-api/src/lib.rs
//! `-- session-api/src/operations.rs
//! Invariants: schema owns the wire shape.
//! Contract tests: session-api/tests/wire.rs

pub use opencode_schema::revert::{FileDiff, State as RevertState};
pub use opencode_schema::session::{
    Direction as ListDirection, Info as SessionInfo, ListAnchor, Time as SessionTime,
};
