//! Purpose: `commitDurableEvent` (packages/core/src/event.ts:205-372): one IMMEDIATE transaction
//! that validates replay coordinates and owners, runs projectors and the local commit hook, then
//! upserts `event_sequence` and inserts the `event` row.
//! Scope: synchronous transaction body run on the storage worker.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- event-store/src/lib.rs
//! |-- event-store/src/read.rs
//! `-- event-store/src/store.rs
//! Invariants: error messages are byte-identical to the baseline. Exact duplicate replay (same id,
//! versioned type and deep-equal encoded data) is a no-op that only backfills a missing owner.
//! A non-strict replay from another owner is skipped silently. The owner column is only written
//! when it was previously null. Any error rolls the whole transaction back.
//! Contract tests: event-store/tests/commit.rs, event-store/tests/replay.rs

use opencode_event_api::{
    BoxError, CommitHook, Definition, Durable, EventError, Payload, Projector, versioned_type,
};
use opencode_storage_api::{Sql, StorageError, Value};
use serde_json::Value as Json;

/// Replay coordinates (`input` of `commitDurableEvent`).
pub(crate) struct ReplayInput {
    pub seq: i64,
    pub aggregate_id: String,
    pub owner_id: Option<String>,
    pub strict_owner: bool,
}

/// Transaction failure. Storage errors keep their classification for `EventError::Storage`.
#[derive(Debug)]
pub(crate) enum CommitError {
    Event(EventError),
    Storage(StorageError),
}

impl From<StorageError> for CommitError {
    fn from(error: StorageError) -> Self {
        CommitError::Storage(error)
    }
}

impl From<EventError> for CommitError {
    fn from(error: EventError) -> Self {
        CommitError::Event(error)
    }
}

impl From<CommitError> for EventError {
    fn from(error: CommitError) -> Self {
        match error {
            CommitError::Event(error) => error,
            CommitError::Storage(error) => storage_error(error),
        }
    }
}

pub(crate) fn storage_error(error: StorageError) -> EventError {
    match error {
        StorageError::Cancelled => EventError::Cancelled,
        StorageError::Closed => EventError::Closed,
        error => EventError::Storage {
            message: error.to_string(),
            retryable: error.is_retryable(),
            source: Some(Box::new(error)),
        },
    }
}

/// Everything the transaction body needs, moved onto the storage worker.
pub(crate) struct Commit {
    pub definition: Definition,
    pub event: Payload,
    pub input: Option<ReplayInput>,
    pub projectors: Vec<Projector>,
    pub hook: Option<CommitHook>,
}

/// Committed coordinates, `None` when the commit was a no-op (exact replay or fenced owner).
pub(crate) type Committed = Option<(String, i64)>;

/// Extracts and validates the aggregate before opening the transaction.
pub(crate) fn aggregate_of(
    definition: &Definition,
    event: &Payload,
    input: Option<&ReplayInput>,
) -> Result<String, EventError> {
    let durable = definition
        .durable
        .expect("aggregate_of is only called for durable definitions");
    let Some(aggregate_id) = event
        .data
        .get(durable.aggregate)
        .and_then(Json::as_str)
        .map(str::to_owned)
    else {
        return Err(EventError::invalid(
            &event.event_type,
            format!("Expected string aggregate field {}", durable.aggregate),
        ));
    };
    if let Some(input) = input
        && input.aggregate_id != aggregate_id
    {
        return Err(EventError::invalid(
            &event.event_type,
            format!(
                "Aggregate mismatch: expected {}, got {aggregate_id}",
                input.aggregate_id
            ),
        ));
    }
    Ok(aggregate_id)
}

/// Encodes data with the definition codec (`Schema.encodeUnknownSync(definition.data)`).
pub(crate) fn encode(definition: &Definition, data: &Json) -> Result<Json, EventError> {
    match &definition.encode {
        Some(encode) => encode(data).map_err(|message| EventError::Codec {
            event_type: definition.event_type.to_owned(),
            message,
        }),
        None => Ok(data.clone()),
    }
}

impl Commit {
    /// Transaction body. `aggregate_id` and `encoded` were computed by the caller.
    pub(crate) fn run(
        self,
        tx: &dyn Sql,
        aggregate_id: &str,
        encoded: &Json,
    ) -> Result<Committed, CommitError> {
        let Commit {
            definition,
            event,
            input,
            projectors,
            hook,
        } = self;
        let durable = definition.durable.expect("durable definition");
        let stored_type = versioned_type(definition.event_type, durable.version);
        let invalid =
            |message: String| CommitError::Event(EventError::invalid(&event.event_type, message));

        let row = tx.query_row(
            "SELECT seq, owner_id FROM event_sequence WHERE aggregate_id = ?",
            &[aggregate_id.into()],
        )?;
        let (latest, owner) = match &row {
            Some(row) => (
                row.get::<i64>("seq")?,
                row.get::<Option<String>>("owner_id")?,
            ),
            None => (-1, None),
        };

        if let Some(input) = &input {
            if input.strict_owner
                && let Some(owner) = owner.as_deref().filter(|owner| !owner.is_empty())
                && Some(owner) != input.owner_id.as_deref()
            {
                return Err(invalid(format!(
                    "Replay owner mismatch for aggregate {aggregate_id}: expected {owner}, got {}",
                    input.owner_id.as_deref().unwrap_or("none")
                )));
            }
            if input.seq <= latest {
                let stored = tx.query_row(
                    "SELECT id, type, data FROM event WHERE aggregate_id = ? AND seq = ?",
                    &[aggregate_id.into(), input.seq.into()],
                )?;
                if let Some(stored) = stored
                    && stored.get::<String>("id")? == event.id
                    && stored.get::<String>("type")? == stored_type
                    && stored.get_json::<Json>("data")? == *encoded
                {
                    if let Some(owner_id) = input.owner_id.as_deref().filter(|id| !id.is_empty())
                        && owner.is_none()
                    {
                        tx.execute(
                            "UPDATE event_sequence SET owner_id = ? WHERE aggregate_id = ?",
                            &[owner_id.into(), aggregate_id.into()],
                        )?;
                    }
                    return Ok(None);
                }
                return Err(invalid(format!(
                    "Replay diverged at aggregate {aggregate_id} sequence {}",
                    input.seq
                )));
            }
            if let Some(owner) = owner.as_deref().filter(|owner| !owner.is_empty())
                && Some(owner) != input.owner_id.as_deref()
            {
                return Ok(None);
            }
        }

        let seq = input.as_ref().map_or(latest + 1, |input| input.seq);
        if input.is_some() && seq != latest + 1 {
            return Err(invalid(format!(
                "Sequence mismatch for aggregate {aggregate_id}: expected {}, got {seq}",
                latest + 1
            )));
        }
        if let Some(stored) = tx.query_row(
            "SELECT aggregate_id, seq FROM event WHERE id = ?",
            &[event.id.as_str().into()],
        )? {
            return Err(invalid(format!(
                "Event {} already exists at aggregate {} sequence {}",
                event.id,
                stored.get::<String>("aggregate_id")?,
                stored.get::<i64>("seq")?
            )));
        }

        let committed = Payload {
            durable: Some(Durable {
                aggregate_id: aggregate_id.to_owned(),
                seq,
                version: durable.version,
            }),
            ..event
        };
        let projector_error = |source: BoxError| {
            CommitError::Event(EventError::Projector {
                event_type: committed.event_type.clone(),
                source,
            })
        };
        for projector in &projectors {
            projector(tx, &committed).map_err(projector_error)?;
        }
        if let Some(hook) = hook {
            hook(tx, seq).map_err(projector_error)?;
        }

        // Insert stores the raw requested owner; an existing row only gains a truthy owner when its
        // owner was null (`input?.ownerID && row?.ownerID == null`).
        let raw_owner = input.as_ref().and_then(|input| input.owner_id.as_deref());
        let claim = raw_owner.filter(|id| !id.is_empty() && owner.is_none());
        match claim {
            Some(owner_id) => tx.execute(
                "INSERT INTO event_sequence (aggregate_id, seq, owner_id) VALUES (?, ?, ?) \
                 ON CONFLICT (aggregate_id) DO UPDATE SET seq = ?, owner_id = ?",
                &[
                    aggregate_id.into(),
                    seq.into(),
                    owner_id.into(),
                    seq.into(),
                    owner_id.into(),
                ],
            )?,
            None => tx.execute(
                "INSERT INTO event_sequence (aggregate_id, seq, owner_id) VALUES (?, ?, ?) \
                 ON CONFLICT (aggregate_id) DO UPDATE SET seq = ?",
                &[
                    aggregate_id.into(),
                    seq.into(),
                    Value::from(raw_owner),
                    seq.into(),
                ],
            )?,
        };
        tx.execute(
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?, ?, ?, ?, ?)",
            &[
                committed.id.as_str().into(),
                aggregate_id.into(),
                seq.into(),
                stored_type.into(),
                Value::json(encoded)?,
            ],
        )?;
        Ok(Some((aggregate_id.to_owned(), seq)))
    }
}
