//! Purpose: Session context epochs (`packages/core/src/session/context-epoch.ts`): initialize the
//! durable baseline once, then per provider turn reconcile the live system context against the
//! stored snapshot: unchanged keeps the baseline, changes publish one `session.next.context.updated`
//! event whose commit hook advances the snapshot atomically, incompatible snapshots or a compaction
//! newer than the baseline replace the generation, unavailable admitted sources block replacement.
//! Scope: `session_context_epoch` persistence plus the ContextUpdated publication.
//! Uses:
//! rust/crates/
//! |-- context/src/history.rs
//! |-- context/src/source/mod.rs
//! |-- event-api/src/lib.rs
//! |-- schema/src/identifier.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- futures::try_join
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- context/src/lib.rs
//! |-- context/tests/epoch.rs
//! |-- context/tests/history.rs
//! `-- context/tests/support/events.rs
//! Invariants: the snapshot advances only inside the ContextUpdated commit (event and snapshot are
//! one transaction), the baseline text never changes outside replacement, `baseline_seq` is the
//! aggregate's latest event seq at insert (or the compaction seq on post-compaction replacement).
//! Contract tests: context/tests/epoch.rs, context/tests/history.rs

use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use opencode_event_api::{Definition, EventStore, PublishOptions};
use opencode_schema::identifier::{Direction, create};
use opencode_storage_api::{Database, DatabaseExt, Sql, TxOptions, Value};
use serde_json::json;

use crate::history;
use crate::source::{
    self, ContextError, Generation, ReconcileResult, ReplacementResult, Snapshot, SystemContext,
};

/// `SessionEvent.ContextUpdated` (packages/schema/src/session-event.ts:101-109).
pub const CONTEXT_UPDATED: Definition =
    Definition::durable("session.next.context.updated", 1, "sessionID");

/// Baseline text and the seq it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub baseline: String,
    pub baseline_seq: i64,
}

/// Stored epoch row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEpoch {
    pub baseline: String,
    pub snapshot: String,
    pub baseline_seq: i64,
}

/// Create the epoch if absent (`initialize`). Returns `None` when one already exists.
pub async fn initialize<C>(
    db: &dyn Database,
    context: C,
    session_id: &str,
) -> Result<Option<Prepared>, ContextError>
where
    C: Future<Output = Result<SystemContext, ContextError>>,
{
    let id = session_id.to_string();
    if db.read(move |sql| find(sql, &id)).await?.is_some() {
        return Ok(None);
    }
    let generation = source::initialize(&context.await?).await?;
    Ok(Some(insert(db, session_id, generation).await?))
}

/// Reconcile the live context with the stored epoch before a provider turn (`prepare`).
pub async fn prepare<E, C>(
    db: &dyn Database,
    events: &E,
    context: C,
    session_id: &str,
) -> Result<Prepared, ContextError>
where
    E: EventStore + ?Sized,
    C: Future<Output = Result<SystemContext, ContextError>>,
{
    let id = session_id.to_string();
    let stored = db.read(move |sql| -> Result<_, ContextError> {
        Ok((find(sql, &id)?, history::latest_compaction(sql, &id)?))
    });
    let (value, (stored, compaction)) = futures::try_join!(context, stored)?;
    let Some(stored) = stored else {
        let generation = source::initialize(&value).await?;
        return insert(db, session_id, generation).await;
    };

    let snapshot: Snapshot =
        serde_json::from_str(&stored.snapshot).map_err(|error| ContextError::SnapshotDecode {
            session_id: session_id.to_string(),
            details: error.to_string(),
        })?;
    let kept = Prepared {
        baseline: stored.baseline.clone(),
        baseline_seq: stored.baseline_seq,
    };
    // Zero is a valid sequence. Presence, not numeric truthiness, selects replacement.
    let replacement_seq = compaction.filter(|seq| *seq > stored.baseline_seq);
    let result = match replacement_seq {
        Some(_) => match source::replace(&value, &snapshot).await? {
            ReplacementResult::ReplacementReady(g) => ReconcileResult::ReplacementReady(g),
            ReplacementResult::ReplacementBlocked => ReconcileResult::ReplacementBlocked,
        },
        None => source::reconcile(&value, &snapshot).await?,
    };
    match result {
        ReconcileResult::Unchanged | ReconcileResult::ReplacementBlocked => Ok(kept),
        ReconcileResult::ReplacementReady(generation) => {
            replace(db, session_id, replacement_seq, generation).await
        }
        ReconcileResult::Updated { text, snapshot } => {
            publish_update(events, session_id, text, snapshot).await?;
            Ok(kept)
        }
    }
}

/// Delete the epoch so the next turn initializes a new generation (`reset`).
pub async fn reset(db: &dyn Database, session_id: &str) -> Result<(), ContextError> {
    let id = session_id.to_string();
    db.transaction(
        TxOptions::default(),
        move |sql| -> Result<(), ContextError> {
            sql.execute(
                "DELETE FROM session_context_epoch WHERE session_id = ?",
                &[Value::from(id)],
            )?;
            Ok(())
        },
    )
    .await
}

/// Read the epoch row inside an existing unit of work.
pub fn find(sql: &dyn Sql, session_id: &str) -> Result<Option<StoredEpoch>, ContextError> {
    let row = sql.query_row(
        "SELECT baseline, snapshot, baseline_seq FROM session_context_epoch WHERE session_id = ?",
        &[Value::from(session_id)],
    )?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(StoredEpoch {
        baseline: row.get("baseline")?,
        snapshot: row.get("snapshot")?,
        baseline_seq: row.get("baseline_seq")?,
    }))
}

/// `EventV2.latestSequence`: aggregate seq or -1.
pub fn latest_sequence(sql: &dyn Sql, aggregate_id: &str) -> Result<i64, ContextError> {
    let row = sql.query_row(
        "SELECT seq FROM event_sequence WHERE aggregate_id = ?",
        &[Value::from(aggregate_id)],
    )?;
    Ok(row.map(|r| r.get::<i64>("seq")).transpose()?.unwrap_or(-1))
}

async fn insert(
    db: &dyn Database,
    session_id: &str,
    generation: Generation,
) -> Result<Prepared, ContextError> {
    let id = session_id.to_string();
    db.transaction(TxOptions::immediate(), move |sql| -> Result<Prepared, ContextError> {
        let baseline_seq = latest_sequence(sql, &id)?;
        sql.execute(
            "INSERT INTO session_context_epoch (session_id, baseline, snapshot, baseline_seq) VALUES (?, ?, ?, ?)",
            &[
                Value::from(id.as_str()),
                Value::from(generation.baseline.as_str()),
                Value::json(&generation.snapshot)?,
                Value::Integer(baseline_seq),
            ],
        )?;
        Ok(Prepared { baseline: generation.baseline, baseline_seq })
    })
    .await
}

async fn replace(
    db: &dyn Database,
    session_id: &str,
    replacement_seq: Option<i64>,
    generation: Generation,
) -> Result<Prepared, ContextError> {
    let id = session_id.to_string();
    db.transaction(TxOptions::immediate(), move |sql| -> Result<Prepared, ContextError> {
        let baseline_seq = match replacement_seq {
            Some(seq) => seq,
            None => latest_sequence(sql, &id)?,
        };
        let updated = sql.execute(
            "UPDATE session_context_epoch SET baseline = ?, snapshot = ?, baseline_seq = ? WHERE session_id = ?",
            &[
                Value::from(generation.baseline.as_str()),
                Value::json(&generation.snapshot)?,
                Value::Integer(baseline_seq),
                Value::from(id.as_str()),
            ],
        )?;
        if updated == 0 {
            return Err(ContextError::EpochNotFound { session_id: id });
        }
        Ok(Prepared { baseline: generation.baseline, baseline_seq })
    })
    .await
}

async fn publish_update<E>(
    events: &E,
    session_id: &str,
    text: String,
    snapshot: Snapshot,
) -> Result<(), ContextError>
where
    E: EventStore + ?Sized,
{
    let encoded = Value::json(&snapshot)?;
    let id = session_id.to_string();
    let data = json!({
        "timestamp": now_millis(),
        "sessionID": session_id,
        "messageID": create("msg", Direction::Ascending, None),
        "text": text,
    });
    let commit: opencode_event_api::CommitHook = Box::new(move |sql, _seq| {
        let updated = sql.execute(
            "UPDATE session_context_epoch SET snapshot = ? WHERE session_id = ?",
            &[encoded, Value::from(id.as_str())],
        )?;
        if updated == 0 {
            return Err(Box::new(ContextError::EpochNotFound { session_id: id }));
        }
        Ok(())
    });
    let options = PublishOptions {
        commit: Some(commit),
        ..Default::default()
    };
    match events.publish(&CONTEXT_UPDATED, data, options).await {
        Ok(_) => Ok(()),
        Err(error) => match error.projector_source::<ContextError>() {
            Some(ContextError::EpochNotFound { session_id }) => Err(ContextError::EpochNotFound {
                session_id: session_id.clone(),
            }),
            _ => Err(error.into()),
        },
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
