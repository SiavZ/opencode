//! Purpose: crate root for the SQLite-backed durable event store (`EventV2` service): commit with
//! projectors in one transaction, replay with owner fencing, claims, durable cursors and
//! post-commit subscribers.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- event-store/src/commit.rs
//! |-- event-store/src/hub.rs
//! |-- event-store/src/read.rs
//! `-- event-store/src/store.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- event-store/tests/rows.rs
//! |-- event-store/tests/streams.rs
//! |-- event-store/tests/support/mod.rs
//! |-- locations/tests/support/workspace.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/process.rs
//! `-- session-runtime/tests/support/sql.rs
//! Invariants: `SqlEventStore` implements `EventStore` over any `Database`.
//! Contract tests: event-store/tests/

mod commit;
mod hub;
mod read;
mod store;

pub use store::{BeforeAggregateRead, Options, SqlEventStore};
