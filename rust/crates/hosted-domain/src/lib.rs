//! Purpose: crate root for portable hosted product rules. Phase 12 owns `console`, Phase 13 owns
//! `sharing`, `analytics` and `sites`. No cloud SDK, database driver or UI types here.
//! Scope: pure domain values and decisions, no I/O.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/analytics/mod.rs
//! |-- hosted-domain/src/console/mod.rs
//! |-- hosted-domain/src/sharing/mod.rs
//! `-- hosted-domain/src/sites/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: depends only on `schema` (contracts/crate-edges.toml), portable to wasm32.
//! Contract tests: none yet.

pub mod analytics;
pub mod console;
pub mod sharing;
pub mod sites;
