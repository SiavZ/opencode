//! Purpose: durable event store service contract (`packages/core/src/event.ts:116-140`).
//! Scope: object-safe async trait. Projectors and hooks receive the storage executor `&dyn Sql`.
//! Uses:
//! rust/crates/
//! |-- event-api/src/definition.rs
//! |-- event-api/src/error.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- futures::stream::BoxStream
//! Used by:
//! rust/crates/
//! `-- event-api/src/lib.rs
//! Invariants: projectors and the commit hook run inside the same immediate transaction as the event
//! and sequence rows, before they are written. Any failure rolls back everything. Durable wakes and
//! listener notification happen only after commit. A replay owner claim is not an execution lease.
//! Contract tests: event-store/tests/commit.rs, event-store/tests/replay.rs

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use opencode_storage_api::Sql;
use serde_json::{Map, Value};

use crate::definition::{Definition, LocationRef, Payload, SerializedEvent};
use crate::error::{BoxError, EventError};

/// Projector run inside the commit transaction. The elided lifetimes are higher-ranked, so any
/// executor borrowed for one unit of work (`FnOnce(&dyn Sql)` in storage-api) can be passed.
pub type Projector = Arc<dyn Fn(&dyn Sql, &Payload) -> Result<(), BoxError> + Send + Sync>;

/// Local operational projection committed with a new durable event, given the assigned seq.
/// Not replayed or serialized (`PublishOptions.commit`).
pub type CommitHook = Box<dyn FnOnce(&dyn Sql, i64) -> Result<(), BoxError> + Send>;

/// Process-local listener. Durable-event listener failures are logged and isolated.
pub type Listener = Arc<dyn Fn(&Payload) -> Result<(), BoxError> + Send + Sync>;

/// Options for [`EventStore::publish`].
#[derive(Default)]
pub struct PublishOptions {
    pub id: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    pub location: Option<LocationRef>,
    pub commit: Option<CommitHook>,
}

impl std::fmt::Debug for PublishOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishOptions")
            .field("id", &self.id)
            .field("location", &self.location)
            .field("commit", &self.commit.is_some())
            .finish_non_exhaustive()
    }
}

/// Options for replay.
#[derive(Debug, Clone, Default)]
pub struct ReplayOptions {
    /// Notify listeners and subscribers after commit.
    pub publish: bool,
    pub owner_id: Option<String>,
    /// Fail when the aggregate already has a different owner.
    pub strict_owner: bool,
}

/// Page returned by [`EventStore::read_aggregate`].
#[derive(Debug, Clone, PartialEq)]
pub struct AggregatePage {
    pub events: Vec<Payload>,
    pub has_more: bool,
}

/// Stream of payloads. Bounded streams end with `SubscriberOverflow` when they fall behind.
pub type EventStream = BoxStream<'static, Result<Payload, EventError>>;

/// Unsubscribe handle returned by [`EventStore::listen`]. Dropping it does not unsubscribe.
pub type Unsubscribe = Box<dyn FnOnce() + Send>;

#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    /// Publish an event. Durable definitions commit (with projectors and the hook) and return the
    /// payload with `durable` set. Ephemeral definitions only notify; a commit hook on an ephemeral
    /// definition fails with `Local commit hooks require a durable event`.
    async fn publish(
        &self,
        definition: &Definition,
        data: Value,
        options: PublishOptions,
    ) -> Result<Payload, EventError>;

    /// Register a projector for a definition type. Projectors run in registration order.
    fn project(&self, definition: &Definition, projector: Projector);

    /// Process-local stream of events of one type, published after registration.
    fn subscribe(&self, definition: &Definition) -> EventStream;

    /// Process-local stream of every event.
    fn all(&self) -> EventStream;

    /// Bounded variant of `all` (`EventV2.allBounded`): overflow ends the stream with `SubscriberOverflow`.
    fn all_bounded(&self, capacity: usize) -> EventStream;

    /// Durable catch-up: historical events after `after` (default -1), then live events woken by commits.
    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream;

    /// Page of an aggregate restricted to `types` (stored versioned types), ordered by seq.
    async fn read_aggregate(
        &self,
        aggregate_id: &str,
        after: Option<i64>,
        limit: usize,
        types: &[String],
    ) -> Result<AggregatePage, EventError>;

    /// Latest committed seq for an aggregate, `-1` when absent.
    async fn latest_sequence(&self, aggregate_id: &str) -> Result<i64, EventError>;

    /// Deprecated listener API, kept for compatibility with `listen`.
    fn listen(&self, listener: Listener) -> Unsubscribe;

    /// Replay one serialized event (sync import). Exact duplicates are no-ops; divergence fails.
    async fn replay(
        &self,
        event: SerializedEvent,
        options: ReplayOptions,
    ) -> Result<(), EventError>;

    /// Replay contiguous events of one aggregate. Each event commits separately. Returns the aggregate ID.
    async fn replay_all(
        &self,
        events: Vec<SerializedEvent>,
        options: ReplayOptions,
    ) -> Result<Option<String>, EventError>;

    /// Delete an aggregate's sequence and events in one transaction.
    async fn remove(&self, aggregate_id: &str) -> Result<(), EventError>;

    /// Set the replay owner of an existing aggregate. Absent aggregates are unchanged.
    async fn claim(&self, aggregate_id: &str, owner_id: &str) -> Result<(), EventError>;
}
