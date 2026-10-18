//! Purpose: read paths of the event store: `readAfter` decoding, the `durable()` catch-up then
//! live stream, `readAggregate` pages and `latestSequence`.
//! Scope: async reads over `Database::read`.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- event-store/src/commit.rs
//! |-- event-store/src/hub.rs
//! |-- event-store/src/store.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- futures::stream
//! `-- serde_json
//! Used by:
//! rust/crates/
//! `-- event-store/src/store.rs
//! Invariants: the durable wake is registered before the historical read, so a commit landing
//! during catch-up leaves a permit and is read on the next pass (no gap, no duplicate because the
//! cursor only advances past rows already yielded). Dropping the stream unregisters the wake.
//! Contract tests: event-store/tests/streams.rs
//! Contract tests: event-store/tests/rows.rs

use std::collections::VecDeque;
use std::sync::Arc;

use futures::StreamExt;
use opencode_event_api::{AggregatePage, Durable, EventError, EventStream, Payload};
use opencode_storage_api::{DatabaseExt, Row, Sql, StorageError, Value};
use serde_json::Value as Json;

use crate::commit::storage_error;
use crate::hub::WakeGuard;
use crate::store::Inner;

struct RawEvent {
    id: String,
    aggregate_id: String,
    seq: i64,
    stored_type: String,
    data: Json,
}

fn raw(row: &Row) -> Result<RawEvent, StorageError> {
    Ok(RawEvent {
        id: row.get("id")?,
        aggregate_id: row.get("aggregate_id")?,
        seq: row.get("seq")?,
        stored_type: row.get("type")?,
        data: row.get_json("data")?,
    })
}

impl Inner {
    /// `decodeSerializedEvent`: unknown stored types fail with `Unknown durable event type`.
    fn decode_serialized(&self, event: RawEvent) -> Result<Payload, EventError> {
        let Some(definition) = self.definition(&event.stored_type) else {
            return Err(EventError::invalid(
                &event.stored_type,
                format!("Unknown durable event type {}", event.stored_type),
            ));
        };
        let durable = definition
            .durable
            .expect("registered definitions are durable");
        Ok(Payload {
            id: event.id,
            metadata: None,
            event_type: definition.event_type.to_owned(),
            durable: Some(Durable {
                aggregate_id: event.aggregate_id,
                seq: event.seq,
                version: durable.version,
            }),
            location: None,
            data: crate::store::decode(&definition, event.data)?,
        })
    }

    /// `readAfter(aggregateID, after)`.
    async fn read_after(&self, aggregate_id: &str, after: i64) -> Result<Vec<Payload>, EventError> {
        if let Some(hook) = &self.before_aggregate_read {
            hook(aggregate_id.to_owned()).await;
        }
        let aggregate = aggregate_id.to_owned();
        let rows = self
            .db
            .read(move |sql: &dyn Sql| {
                sql.query(
                    "SELECT id, aggregate_id, seq, type, data FROM event \
                     WHERE aggregate_id = ? AND seq > ? ORDER BY seq ASC",
                    &[aggregate.into(), after.into()],
                )?
                .iter()
                .map(raw)
                .collect::<Result<Vec<_>, StorageError>>()
            })
            .await
            .map_err(storage_error)?;
        rows.into_iter()
            .map(|row| self.decode_serialized(row))
            .collect()
    }

    pub(crate) async fn latest_sequence(&self, aggregate_id: &str) -> Result<i64, EventError> {
        let aggregate = aggregate_id.to_owned();
        self.db
            .read(move |sql: &dyn Sql| {
                Ok::<_, StorageError>(
                    match sql.query_row(
                        "SELECT seq FROM event_sequence WHERE aggregate_id = ?",
                        &[aggregate.into()],
                    )? {
                        Some(row) => row.get::<i64>("seq")?,
                        None => -1,
                    },
                )
            })
            .await
            .map_err(storage_error)
    }

    /// `readAggregate`: `limit + 1` rows decide `has_more`; unknown types keep their stored name.
    pub(crate) async fn read_aggregate(
        &self,
        aggregate_id: &str,
        after: Option<i64>,
        limit: usize,
        types: &[String],
    ) -> Result<AggregatePage, EventError> {
        if types.is_empty() {
            return Ok(AggregatePage {
                events: vec![],
                has_more: false,
            });
        }
        let mut params: Vec<Value> = vec![aggregate_id.into(), after.unwrap_or(-1).into()];
        params.extend(types.iter().map(|value| Value::from(value.as_str())));
        params.push(
            i64::try_from(limit.saturating_add(1))
                .unwrap_or(i64::MAX)
                .into(),
        );
        let sql = format!(
            "SELECT id, aggregate_id, seq, type, data FROM event \
             WHERE aggregate_id = ? AND seq > ? AND type IN ({}) ORDER BY seq ASC LIMIT ?",
            vec!["?"; types.len()].join(", ")
        );
        let rows = self
            .db
            .read(move |tx: &dyn Sql| {
                tx.query(&sql, &params)?
                    .iter()
                    .map(raw)
                    .collect::<Result<Vec<_>, StorageError>>()
            })
            .await
            .map_err(storage_error)?;
        let has_more = rows.len() > limit;
        let events = rows
            .into_iter()
            .take(limit)
            .map(|row| {
                let definition = self.definition(&row.stored_type);
                let version = definition
                    .as_ref()
                    .and_then(|definition| definition.durable)
                    .map(|durable| durable.version)
                    .or_else(|| version_suffix(&row.stored_type))
                    .unwrap_or_default();
                let data = match &definition {
                    Some(definition) => crate::store::decode(definition, row.data)?,
                    None => row.data,
                };
                Ok(Payload {
                    id: row.id,
                    metadata: None,
                    event_type: definition
                        .map(|definition| definition.event_type.to_owned())
                        .unwrap_or(row.stored_type),
                    durable: Some(Durable {
                        aggregate_id: row.aggregate_id,
                        seq: row.seq,
                        version,
                    }),
                    location: None,
                    data,
                })
            })
            .collect::<Result<Vec<_>, EventError>>()?;
        Ok(AggregatePage { events, has_more })
    }
}

fn version_suffix(stored_type: &str) -> Option<u32> {
    stored_type.rsplit_once('.')?.1.parse().ok()
}

struct DurableState {
    inner: Arc<Inner>,
    guard: WakeGuard,
    aggregate_id: String,
    sequence: i64,
    buffer: VecDeque<Payload>,
    started: bool,
    failed: bool,
}

/// `durable({ aggregateID, after })`: historical rows, then one re-read per (coalesced) wake.
pub(crate) fn durable(inner: Arc<Inner>, aggregate_id: &str, after: Option<i64>) -> EventStream {
    let state = DurableState {
        guard: inner.hub.durable_wake(aggregate_id),
        inner,
        aggregate_id: aggregate_id.to_owned(),
        sequence: after.unwrap_or(-1),
        buffer: VecDeque::new(),
        started: false,
        failed: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if state.failed {
                return None;
            }
            if let Some(event) = state.buffer.pop_front() {
                return Some((Ok(event), state));
            }
            if state.started {
                state.guard.notify.notified().await;
            }
            state.started = true;
            match state
                .inner
                .read_after(&state.aggregate_id, state.sequence)
                .await
            {
                Ok(events) => {
                    if let Some(seq) = events
                        .last()
                        .and_then(|event| event.durable.as_ref())
                        .map(|durable| durable.seq)
                    {
                        state.sequence = seq;
                    }
                    state.buffer.extend(events);
                }
                Err(error) => {
                    state.failed = true;
                    return Some((Err(error), state));
                }
            }
        }
    })
    .boxed()
}
