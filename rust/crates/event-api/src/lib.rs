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
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/src/source/error.rs
//! |-- context/tests/epoch.rs
//! |-- context/tests/history.rs
//! |-- context/tests/support/events.rs
//! |-- context/tests/support/mod.rs
//! |-- event-store/src/commit.rs
//! |-- event-store/src/hub.rs
//! |-- event-store/src/read.rs
//! |-- event-store/src/store.rs
//! |-- event-store/tests/commit.rs
//! |-- event-store/tests/owners.rs
//! |-- event-store/tests/replay.rs
//! |-- event-store/tests/rows.rs
//! |-- event-store/tests/streams.rs
//! |-- event-store/tests/support/mod.rs
//! |-- locations/src/error.rs
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! |-- locations/tests/workspace_sync.rs
//! |-- runtime/src/legacy.rs
//! |-- server/src/services/streams.rs
//! |-- server/tests/sse_events.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/children/store.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/projection/adapter.rs
//! |-- session-runtime/src/projection/legacy.rs
//! |-- session-runtime/src/projection/updater.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/todo.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_projection.rs
//! |-- session-runtime/tests/support/publish.rs
//! `-- session-runtime/tests/todo.rs
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
