//! Purpose: crate root for versioned durable event contracts: definitions, payloads, serialized rows,
//! publish/replay/cursor/claim service trait and typed errors.
//! Scope: pure values and async service traits. No storage engine.
//! Uses:
//! rust/crates/
//! |-- event-api/src/definition.rs
//! |-- event-api/src/error.rs
//! `-- event-api/src/store.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-02, consumers 04/06 added as crates land)
//! Invariants: portable (native + wasm). Generic over the transaction context so it needs no storage edge.
//! Contract tests: event-api/src/definition.rs, event-store/tests/commit.rs

pub mod definition;
pub mod error;
pub mod store;

pub use definition::{
    Codec, Definition, Durable, DurableSpec, EVENT_ID_PREFIX, LocationRef, Payload,
    SerializedEvent, versioned_type,
};
pub use error::{BoxError, EventError};
pub use store::{
    AggregatePage, CommitHook, EventStore, EventStream, Listener, Projector, PublishOptions,
    ReplayOptions, Unsubscribe,
};
