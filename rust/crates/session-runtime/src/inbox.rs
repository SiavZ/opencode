//! Purpose: the durable Session inbox (`session_input`), porting packages/core/src/session/input.ts:
//! row lookup, the `prompt.admitted` and `prompted` projections, pending checks and promotion of
//! steers (through a captured cutoff) and the next queued input. Seams consumed by the p04 runner.
//! Scope: projections are synchronous inside the publish transaction; promotions publish events.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/session_event.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- serde_json
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/tests/admission.rs
//! `-- session-runtime/tests/admission_inbox.rs
//! Invariants: a row is admitted once (unique id, unique (session, admitted_seq)) and promoted once
//! (`promoted_seq IS NULL` guard). A visible message with the same ID blocks admission
//! (`LifecycleConflict`). A promotion that lost a race to another promoter is a no-op.
//! Contract tests: session-runtime/tests/admission_inbox.rs, session-runtime/tests/admission.rs

use opencode_event_api::{Payload, PublishOptions};
use opencode_schema::session_event::{PromptAdmitted, Prompted};
use opencode_schema::values::{DateTimeUtc, NonNegativeInt};
use opencode_session_api::admission::{delivery_str, equivalent, parse_delivery};
use opencode_session_api::{Admitted, Delivery, MessageID, Prompt, SessionError, SessionID};
use opencode_storage_api::{DatabaseExt, Row, Sql, StorageError, Value};

use crate::projection::{
    ProjectionError, SessionEvents, decode, event_error, publish, storage_error,
};

const COLUMNS: &str = "id, session_id, prompt, delivery, admitted_seq, promoted_seq, time_created";

/// Decodes one `session_input` row (`fromRow`).
pub fn from_row(row: &Row) -> Result<Admitted, StorageError> {
    let invalid = |column: &str, message: String| StorageError::Decode {
        column: column.into(),
        message,
    };
    let delivery: String = row.get("delivery")?;
    let id: String = row.get("id")?;
    let session: String = row.get("session_id")?;
    let seq = |column: &str, value: i64| {
        NonNegativeInt::new(value).ok_or_else(|| invalid(column, format!("negative seq {value}")))
    };
    let promoted: Option<i64> = row.get("promoted_seq")?;
    Ok(Admitted {
        admitted_seq: seq("admitted_seq", row.get("admitted_seq")?)?,
        id: MessageID::new(id).map_err(|error| invalid("id", error.to_string()))?,
        session_id: SessionID::new(session)
            .map_err(|error| invalid("session_id", error.to_string()))?,
        prompt: row.get_json("prompt")?,
        delivery: parse_delivery(&delivery)
            .ok_or_else(|| invalid("delivery", format!("unknown delivery {delivery}")))?,
        time_created: millis(row.get("time_created")?)
            .ok_or_else(|| invalid("time_created", "out of range".into()))?,
        promoted_seq: promoted
            .map(|value| seq("promoted_seq", value))
            .transpose()?,
    })
}

fn millis(value: i64) -> Option<DateTimeUtc> {
    DateTimeUtc::from_millis(value)
}

/// `SessionInput.find`.
pub fn find(tx: &dyn Sql, id: &str) -> Result<Option<Admitted>, StorageError> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM session_input WHERE id = ?"),
        &[id.into()],
    )?
    .as_ref()
    .map(from_row)
    .transpose()
}

/// Any unpromoted input of this delivery for the Session (`SessionInput.hasPending`).
pub fn has_pending(
    tx: &dyn Sql,
    session: &SessionID,
    delivery: Delivery,
) -> Result<bool, SessionError> {
    tx.query_row(
        "SELECT id FROM session_input WHERE session_id = ? AND promoted_seq IS NULL AND delivery = ? LIMIT 1",
        &[session.as_str().into(), delivery_str(delivery).into()],
    )
    .map(|row| row.is_some())
    .map_err(storage_error)
}

/// Publish `session.next.prompted` for every pending steer with admitted_seq <= cutoff, ascending. Returns count.
pub async fn promote_steers(
    events: &dyn SessionEvents,
    session: &SessionID,
    cutoff: i64,
) -> Result<usize, SessionError> {
    let session_id = session.as_str().to_owned();
    let rows = events
        .database()
        .read(move |tx: &dyn Sql| {
            tx.query(
                &format!(
                    "SELECT {COLUMNS} FROM session_input WHERE session_id = ? AND promoted_seq IS NULL AND delivery = 'steer' AND admitted_seq <= ? ORDER BY admitted_seq ASC"
                ),
                &[session_id.into(), Value::Integer(cutoff)],
            )?
            .iter()
            .map(from_row)
            .collect::<Result<Vec<_>, StorageError>>()
        })
        .await
        .map_err(storage_error)?;
    promote(events, session, rows).await
}

/// Publish `session.next.prompted` for the oldest pending queue input. Returns whether one was promoted.
pub async fn promote_next_queued(
    events: &dyn SessionEvents,
    session: &SessionID,
) -> Result<bool, SessionError> {
    let session_id = session.as_str().to_owned();
    let row = events
        .database()
        .read(move |tx: &dyn Sql| {
            tx.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM session_input WHERE session_id = ? AND promoted_seq IS NULL AND delivery = 'queue' ORDER BY admitted_seq ASC LIMIT 1"
                ),
                &[session_id.into()],
            )?
            .as_ref()
            .map(from_row)
            .transpose()
        })
        .await
        .map_err(storage_error)?;
    match row {
        None => Ok(false),
        Some(row) => promote(events, session, vec![row]).await.map(|_| true),
    }
}

/// `publish`: one `Prompted` per row. A `LifecycleConflict` is tolerated only when another promoter
/// already promoted the row.
async fn promote(
    events: &dyn SessionEvents,
    session: &SessionID,
    rows: Vec<Admitted>,
) -> Result<usize, SessionError> {
    let count = rows.len();
    for row in rows {
        let data = Prompted {
            timestamp: row.time_created,
            session_id: session.clone(),
            message_id: row.id.clone(),
            prompt: row.prompt,
            delivery: row.delivery,
        };
        let Err(error) = publish(events, &data, PublishOptions::default()).await else {
            continue;
        };
        if error
            .projector_source::<ProjectionError>()
            .is_none_or(|source| !matches!(source, ProjectionError::LifecycleConflict { .. }))
        {
            return Err(event_error(error));
        }
        let id = row.id.as_str().to_owned();
        let stored = events
            .database()
            .read(move |tx: &dyn Sql| find(tx, &id))
            .await
            .map_err(storage_error)?;
        if stored.and_then(|stored| stored.promoted_seq).is_none() {
            return Err(event_error(error));
        }
    }
    Ok(count)
}

/// `SessionInput.projectAdmitted`: record a pending inbox row at the event's seq.
pub(crate) fn project_admitted(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let seq = payload
        .durable
        .as_ref()
        .ok_or(ProjectionError::MissingSequence)?
        .seq;
    let e: PromptAdmitted = decode(payload)?;
    let conflict = || ProjectionError::LifecycleConflict {
        id: e.message_id.clone(),
    };
    if tx
        .query_row(
            "SELECT id FROM session_message WHERE id = ?",
            &[e.message_id.as_str().into()],
        )?
        .is_some()
    {
        return Err(conflict());
    }
    let stored = tx.query_row(
        "INSERT INTO session_input (id, session_id, admitted_seq, prompt, delivery, time_created) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING RETURNING id",
        &[
            e.message_id.as_str().into(),
            e.session_id.as_str().into(),
            Value::Integer(seq),
            Value::json(&e.prompt)?,
            delivery_str(e.delivery).into(),
            Value::Integer(e.timestamp.millis()),
        ],
    )?;
    if stored.is_none() {
        return Err(conflict());
    }
    Ok(())
}

/// `SessionInput.projectPrompted`: mark the admitted row promoted, or (replay of history recorded
/// before the inbox existed) insert an already-promoted row.
pub(crate) fn project_prompted(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let seq = payload
        .durable
        .as_ref()
        .ok_or(ProjectionError::MissingSequence)?
        .seq;
    let e: Prompted = decode(payload)?;
    let conflict = || ProjectionError::LifecycleConflict {
        id: e.message_id.clone(),
    };
    let matches = |stored: &Admitted| {
        equivalent(stored, &e.session_id, &e.prompt, e.delivery)
            && stored.time_created.millis() == e.timestamp.millis()
    };
    let updated = tx.query_row(
        &format!(
            "UPDATE session_input SET promoted_seq = ? WHERE id = ? AND session_id = ? AND promoted_seq IS NULL RETURNING {COLUMNS}"
        ),
        &[
            Value::Integer(seq),
            e.message_id.as_str().into(),
            e.session_id.as_str().into(),
        ],
    )?;
    if let Some(row) = updated {
        return if matches(&from_row(&row)?) {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    if let Some(stored) = find(tx, e.message_id.as_str())? {
        let same_seq = stored.promoted_seq.map(NonNegativeInt::get) == Some(seq);
        return if matches(&stored) && same_seq {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    insert_promoted(
        tx,
        &e.message_id,
        &e.session_id,
        &e.prompt,
        e.delivery,
        seq,
        e.timestamp.millis(),
    )?;
    Ok(())
}

/// Inserts an inbox row that is already promoted at `seq` (admitted and promoted at the same seq).
pub(crate) fn insert_promoted(
    tx: &dyn Sql,
    id: &MessageID,
    session: &SessionID,
    prompt: &Prompt,
    delivery: Delivery,
    seq: i64,
    time_created: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO session_input (id, session_id, prompt, delivery, admitted_seq, promoted_seq, time_created) VALUES (?, ?, ?, ?, ?, ?, ?)",
        &[
            id.as_str().into(),
            session.as_str().into(),
            Value::json(prompt)?,
            delivery_str(delivery).into(),
            Value::Integer(seq),
            Value::Integer(seq),
            Value::Integer(time_created),
        ],
    )?;
    Ok(())
}
