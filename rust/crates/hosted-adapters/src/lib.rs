//! Purpose: crate root for native hosted adapters that implement hosted-domain ports. Phase 12
//! owns `console`, Phase 13 owns `sharing`, `analytics` and `sites`.
//! Scope: I/O adapters (MySQL, HTTP clients, signing), no product rules.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/analytics/mod.rs
//! |-- hosted-adapters/src/console/mod.rs
//! |-- hosted-adapters/src/sharing/mod.rs
//! `-- hosted-adapters/src/sites/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- hosted/src/main.rs
//! Invariants: console MySQL access uses sqlx (mysql, runtime-tokio) only.
//! Contract tests: none yet.

pub mod analytics;
pub mod console;
pub mod sharing;
pub mod sites;
