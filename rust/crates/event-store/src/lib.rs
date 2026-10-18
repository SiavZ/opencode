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
//! `-- none, reserved contract (owner: phase-02, composed by runtime in phase 91)
//! Invariants: `SqlEventStore` implements `EventStore` over any `Database`.
//! Contract tests: event-store/tests/

mod commit;
mod hub;
mod read;
mod store;

pub use store::{BeforeAggregateRead, Options, SqlEventStore};
