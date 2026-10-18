//! Purpose: `EventStore` implementation over `Arc<dyn Database>` (`EventV2.layerWith`): publish,
//! projectors, replay, claims and removal. Reads and streams live in read.rs, fan-out in hub.rs.
//! Scope: native, tokio. Transactions run on the storage worker; no lock held across awaits.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/commit.rs
//! |-- event-store/src/hub.rs
//! |-- event-store/src/read.rs
//! |-- schema/src/identifier.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::future::BoxFuture
//! `-- serde_json
//! Used by:
//! rust/crates/
//! `-- event-store/src/lib.rs
//! Invariants: durable commits use one IMMEDIATE transaction; wakes and notifications happen only
//! after commit. Projectors run in registration order. The stored-type registry contains every
//! durable definition passed to `register`, `project` or `publish`; replay of an unregistered
//! type fails with `Unknown durable event type`.
//! Contract tests: event-store/tests/commit.rs, event-store/tests/replay.rs, event-store/tests/streams.rs

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures::future::BoxFuture;
use opencode_event_api::{
    AggregatePage, Definition, Durable, EVENT_ID_PREFIX, EventError, EventStore, EventStream,
    Listener, Payload, Projector, PublishOptions, ReplayOptions, SerializedEvent, Unsubscribe,
};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, TxOptions};
use serde_json::Value as Json;

use crate::commit::{Commit, Committed, ReplayInput, aggregate_of, encode, storage_error};
use crate::hub::Hub;

/// Hook awaited before each aggregate read of `durable()` (`LayerOptions.beforeAggregateRead`).
pub type BeforeAggregateRead = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

/// Construction options (`EventV2.LayerOptions`).
#[derive(Clone, Default)]
pub struct Options {
    pub before_aggregate_read: Option<BeforeAggregateRead>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field(
                "before_aggregate_read",
                &self.before_aggregate_read.is_some(),
            )
            .finish()
    }
}

pub(crate) struct Inner {
    pub(crate) db: Arc<dyn Database>,
    pub(crate) hub: Arc<Hub>,
    projectors: RwLock<HashMap<String, Vec<Projector>>>,
    /// Durable definitions keyed by stored versioned type (`Durable.get`).
    definitions: RwLock<HashMap<String, Definition>>,
    pub(crate) before_aggregate_read: Option<BeforeAggregateRead>,
}

/// SQLite-backed `EventStore` (`EventV2.Service`). Cheap to clone.
#[derive(Clone)]
pub struct SqlEventStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SqlEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEventStore").finish_non_exhaustive()
    }
}

impl SqlEventStore {
    pub fn new(db: Arc<dyn Database>) -> Self {
        Self::with_options(db, Options::default())
    }

    pub fn with_options(db: Arc<dyn Database>, options: Options) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                hub: Arc::new(Hub::default()),
                projectors: RwLock::default(),
                definitions: RwLock::default(),
                before_aggregate_read: options.before_aggregate_read,
            }),
        }
    }

    /// Registers durable definitions for replay and durable reads (the `Durable` manifest).
    /// Ephemeral definitions are ignored. Re-registering a stored type replaces it.
    pub fn register<'a>(&self, definitions: impl IntoIterator<Item = &'a Definition>) {
        let mut registry = write(&self.inner.definitions);
        for definition in definitions {
            if let Some(stored) = definition.stored_type() {
                registry.insert(stored, definition.clone());
            }
        }
    }

    /// The underlying database handle.
    pub fn database(&self) -> &Arc<dyn Database> {
        &self.inner.db
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}

/// Decodes stored data with the definition codec (`Schema.decodeUnknownSync(definition.data)`).
pub(crate) fn decode(definition: &Definition, data: Json) -> Result<Json, EventError> {
    match &definition.decode {
        Some(decode) => decode(&data).map_err(|message| EventError::Codec {
            event_type: definition.event_type.to_owned(),
            message,
        }),
        None => Ok(data),
    }
}

impl Inner {
    pub(crate) fn definition(&self, stored_type: &str) -> Option<Definition> {
        read(&self.definitions).get(stored_type).cloned()
    }

    fn remember(&self, definition: &Definition) {
        if let Some(stored) = definition.stored_type()
            && !read(&self.definitions).contains_key(&stored)
        {
            write(&self.definitions).insert(stored, definition.clone());
        }
    }

    /// `commitDurableEvent`, then the post-commit durable wake.
    async fn commit(
        &self,
        definition: &Definition,
        event: Payload,
        input: Option<ReplayInput>,
        hook: Option<opencode_event_api::CommitHook>,
    ) -> Result<Committed, EventError> {
        let aggregate_id = aggregate_of(definition, &event, input.as_ref())?;
        let encoded = encode(definition, &event.data)?;
        let projectors = read(&self.projectors)
            .get(definition.event_type)
            .cloned()
            .unwrap_or_default();
        let commit = Commit {
            definition: definition.clone(),
            event,
            input,
            projectors,
            hook,
        };
        let committed = self
            .db
            .transaction(TxOptions::immediate(), move |tx: &dyn Sql| {
                commit.run(tx, &aggregate_id, &encoded)
            })
            .await
            .map_err(EventError::from)?;
        if let Some((aggregate_id, _)) = &committed {
            self.hub.wake(aggregate_id);
        }
        Ok(committed)
    }

    async fn replay(
        &self,
        event: SerializedEvent,
        options: &ReplayOptions,
    ) -> Result<(), EventError> {
        let Some(definition) = self.definition(&event.event_type) else {
            return Err(EventError::invalid(
                &event.event_type,
                format!("Unknown durable event type {}", event.event_type),
            ));
        };
        let version = definition.durable.expect("registered durable").version;
        let payload = Payload {
            id: event.id,
            metadata: None,
            event_type: definition.event_type.to_owned(),
            durable: None,
            location: None,
            data: decode(&definition, event.data)?,
        };
        let input = ReplayInput {
            seq: event.seq,
            aggregate_id: event.aggregate_id,
            owner_id: options.owner_id.clone(),
            strict_owner: options.strict_owner,
        };
        let committed = self
            .commit(&definition, payload.clone(), Some(input), None)
            .await?;
        if let Some((aggregate_id, seq)) = committed
            && options.publish
        {
            let published = Payload {
                durable: Some(Durable {
                    aggregate_id,
                    seq,
                    version,
                }),
                ..payload
            };
            self.hub
                .notify(&published, true)
                .expect("isolated notification never fails");
        }
        Ok(())
    }
}

#[async_trait]
impl EventStore for SqlEventStore {
    async fn publish(
        &self,
        definition: &Definition,
        data: Json,
        options: PublishOptions,
    ) -> Result<Payload, EventError> {
        let event = Payload {
            id: options.id.unwrap_or_else(|| {
                format!(
                    "{EVENT_ID_PREFIX}{}",
                    opencode_schema::identifier::ascending()
                )
            }),
            metadata: options.metadata,
            event_type: definition.event_type.to_owned(),
            durable: None,
            location: options.location,
            data,
        };
        let Some(durable) = definition.durable else {
            if options.commit.is_some() {
                return Err(EventError::invalid(
                    &event.event_type,
                    "Local commit hooks require a durable event",
                ));
            }
            self.inner
                .hub
                .notify(&event, false)
                .map_err(|source| EventError::Projector {
                    event_type: event.event_type.clone(),
                    source,
                })?;
            return Ok(event);
        };
        self.inner.remember(definition);
        let committed = self
            .inner
            .commit(definition, event.clone(), None, options.commit)
            .await?;
        let Some((aggregate_id, seq)) = committed else {
            // Unreachable for publish (no replay input), kept to mirror the baseline fallthrough.
            self.inner
                .hub
                .notify(&event, false)
                .map_err(|source| EventError::Projector {
                    event_type: event.event_type.clone(),
                    source,
                })?;
            return Ok(event);
        };
        let event = Payload {
            durable: Some(Durable {
                aggregate_id,
                seq,
                version: durable.version,
            }),
            ..event
        };
        self.inner
            .hub
            .notify(&event, true)
            .expect("isolated notification never fails");
        Ok(event)
    }

    fn project(&self, definition: &Definition, projector: Projector) {
        self.inner.remember(definition);
        write(&self.inner.projectors)
            .entry(definition.event_type.to_owned())
            .or_default()
            .push(projector);
    }

    fn subscribe(&self, definition: &Definition) -> EventStream {
        self.inner.hub.subscribe(definition.event_type)
    }

    fn all(&self) -> EventStream {
        self.inner.hub.all()
    }

    fn all_bounded(&self, capacity: usize) -> EventStream {
        self.inner.hub.all_bounded(capacity)
    }

    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream {
        crate::read::durable(self.inner.clone(), aggregate_id, after)
    }

    async fn read_aggregate(
        &self,
        aggregate_id: &str,
        after: Option<i64>,
        limit: usize,
        types: &[String],
    ) -> Result<AggregatePage, EventError> {
        self.inner
            .read_aggregate(aggregate_id, after, limit, types)
            .await
    }

    async fn latest_sequence(&self, aggregate_id: &str) -> Result<i64, EventError> {
        self.inner.latest_sequence(aggregate_id).await
    }

    fn listen(&self, listener: Listener) -> Unsubscribe {
        self.inner.hub.listen(listener)
    }

    async fn replay(
        &self,
        event: SerializedEvent,
        options: ReplayOptions,
    ) -> Result<(), EventError> {
        self.inner.replay(event, &options).await
    }

    async fn replay_all(
        &self,
        events: Vec<SerializedEvent>,
        options: ReplayOptions,
    ) -> Result<Option<String>, EventError> {
        let Some(first) = events.first() else {
            return Ok(None);
        };
        let source = first.aggregate_id.clone();
        if source.is_empty() {
            return Ok(None);
        }
        if events.iter().any(|event| event.aggregate_id != source) {
            return Err(EventError::invalid(
                &first.event_type,
                "Replay events must belong to the same aggregate",
            ));
        }
        let start = first.seq;
        for (index, event) in events.iter().enumerate() {
            let seq = start + index as i64;
            if event.seq != seq {
                return Err(EventError::invalid(
                    &event.event_type,
                    format!(
                        "Replay sequence mismatch at index {index}: expected {seq}, got {}",
                        event.seq
                    ),
                ));
            }
        }
        for event in events {
            self.inner.replay(event, &options).await?;
        }
        Ok(Some(source))
    }

    async fn remove(&self, aggregate_id: &str) -> Result<(), EventError> {
        let aggregate = aggregate_id.to_owned();
        self.inner
            .db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(
                    "DELETE FROM event_sequence WHERE aggregate_id = ?",
                    &[aggregate.as_str().into()],
                )?;
                tx.execute(
                    "DELETE FROM event WHERE aggregate_id = ?",
                    &[aggregate.as_str().into()],
                )?;
                Ok::<_, StorageError>(())
            })
            .await
            .map_err(storage_error)
    }

    async fn claim(&self, aggregate_id: &str, owner_id: &str) -> Result<(), EventError> {
        let (aggregate, owner) = (aggregate_id.to_owned(), owner_id.to_owned());
        self.inner
            .db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(
                    "UPDATE event_sequence SET owner_id = ? WHERE aggregate_id = ?",
                    &[owner.into(), aggregate.into()],
                )
                .map(|_| ())
            })
            .await
            .map_err(storage_error)
    }
}
