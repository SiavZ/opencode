//! Purpose: Session event projectors writing `session`, `session_message` and `session_input` rows
//! inside the publish transaction (packages/core/src/session/projector.ts), plus the `SessionEvents`
//! facade and publish helpers shared by admission, runner, lifecycle and legacy modules.
//! Scope: projectors are synchronous closures over `&dyn Sql`; publish helpers are async.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/event.rs
//! |-- schema/src/session_event.rs
//! |-- session-api/src/error.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/projection/adapter.rs
//! |-- session-runtime/src/projection/legacy.rs
//! |-- session-runtime/src/projection/updater.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- serde_json::from_value
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/src/service.rs
//! `-- session-runtime/tests/admission_projection.rs
//! Invariants: every projector write commits or rolls back with its event. Typed projector failures
//! (`ProjectionError`) roll the publish back and are recovered with `EventError::projector_source`.
//! `session.created` for an existing row fails with `SessionAlreadyProjected` (concurrent create).
//! Contract tests: session-runtime/tests/admission_projection.rs, session-runtime/tests/admission.rs

mod adapter;
mod legacy;
mod updater;

use std::sync::Arc;

use opencode_event_api::{Definition, EventError, EventStore, Payload, Projector, PublishOptions};
use opencode_schema::event::EventData;
use opencode_schema::session_event::{
    AgentSwitched, DURABLE_DEFINITIONS, ModelSwitched, Moved, RevertCommitted, RevertStaged,
};
use opencode_schema::values::DateTimeUtc;
use opencode_session_api::{MessageID, SessionError};
use opencode_storage_api::{Database, Sql, StorageError, Value};

use crate::inbox;

/// Typed failure of a Session projector. Boxed through `EventError::Projector`.
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    /// `SessionInput.LifecycleConflict`: the message ID is already visible or admitted differently.
    #[error("Session input lifecycle conflict for {id}")]
    LifecycleConflict { id: MessageID },
    /// `SessionProjector.SessionAlreadyProjected`: `session.created` lost the projection race.
    #[error("Session already projected")]
    SessionAlreadyProjected,
    /// `Revert boundary message not found: <id>`.
    #[error("Revert boundary message not found: {0}")]
    RevertBoundaryMissing(String),
    #[error("Durable Session event is missing aggregate sequence")]
    MissingSequence,
    #[error("failed to decode {event_type}: {message}")]
    Decode { event_type: String, message: String },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Event store and database used by Session admission and promotion.
pub trait SessionEvents: Send + Sync {
    fn database(&self) -> Arc<dyn Database>;
    fn store(&self) -> Arc<dyn EventStore>;
}

/// `SessionEvents` over a process-global database and event store.
#[derive(Clone)]
pub struct LocalSessionEvents {
    database: Arc<dyn Database>,
    store: Arc<dyn EventStore>,
}

impl LocalSessionEvents {
    /// Wraps handles whose Session projectors were already registered.
    pub fn new(database: Arc<dyn Database>, store: Arc<dyn EventStore>) -> Self {
        Self { database, store }
    }

    /// Registers the Session projectors once (`SessionProjector.node`) and wraps the handles.
    pub fn install(database: Arc<dyn Database>, store: Arc<dyn EventStore>) -> Self {
        register_projectors(store.as_ref());
        Self::new(database, store)
    }
}

impl SessionEvents for LocalSessionEvents {
    fn database(&self) -> Arc<dyn Database> {
        self.database.clone()
    }
    fn store(&self) -> Arc<dyn EventStore> {
        self.store.clone()
    }
}

/// event-api definition for a schema `EventData` type.
pub fn definition_of<T: EventData>() -> Definition {
    definition(&T::DEFINITION)
}

fn definition(schema: &opencode_schema::event::Definition) -> Definition {
    match schema.durable {
        Some(durable) => Definition::durable(schema.r#type, durable.version, durable.aggregate),
        None => Definition::ephemeral(schema.r#type),
    }
}

/// Publishes a schema-typed event and returns the committed payload.
pub async fn publish<T: EventData>(
    events: &dyn SessionEvents,
    data: &T,
    options: PublishOptions,
) -> Result<Payload, EventError> {
    let value = serde_json::to_value(data).map_err(|error| EventError::Codec {
        event_type: T::DEFINITION.r#type.into(),
        message: error.to_string(),
    })?;
    events
        .store()
        .publish(&definition_of::<T>(), value, options)
        .await
}

/// Publishes a durable Session event and returns its aggregate seq.
pub async fn publish_session_event<T: EventData>(
    events: &dyn SessionEvents,
    data: &T,
) -> Result<i64, SessionError> {
    let payload = publish(events, data, PublishOptions::default())
        .await
        .map_err(event_error)?;
    payload
        .durable
        .map(|durable| durable.seq)
        .ok_or_else(|| SessionError::Storage {
            message: ProjectionError::MissingSequence.to_string(),
            retryable: false,
        })
}

/// Maps an event failure to a Session storage failure, keeping retryability.
pub fn event_error(error: EventError) -> SessionError {
    SessionError::Storage {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}

pub fn storage_error(error: StorageError) -> SessionError {
    SessionError::Storage {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}

pub(crate) fn now_millis() -> i64 {
    DateTimeUtc::now().millis()
}

/// Registers every Session projector in baseline order (`SessionProjector.layer`).
pub fn register_projectors(store: &dyn EventStore) {
    let legacy = |name: &'static str| Definition::durable(name, 1, "sessionID");
    store.project(
        &legacy("session.created"),
        projector(legacy::project_created),
    );
    store.project(
        &legacy("session.updated"),
        projector(legacy::project_updated),
    );
    store.project(
        &legacy("session.deleted"),
        projector(|tx, payload| {
            tx.execute(
                "DELETE FROM session WHERE id = ?",
                &[session_id(payload)?.into()],
            )?;
            Ok(())
        }),
    );
    store.project(&definition_of::<Moved>(), projector(project_moved));
    store.project(
        &definition_of::<AgentSwitched>(),
        projector(|tx, payload| {
            let e: AgentSwitched = decode(payload)?;
            tx.execute(
                "UPDATE session SET agent = ?, time_updated = ? WHERE id = ?",
                &[
                    e.agent.into(),
                    Value::Integer(e.timestamp.millis()),
                    e.session_id.as_str().into(),
                ],
            )?;
            updater::apply(tx, payload)
        }),
    );
    store.project(
        &definition_of::<ModelSwitched>(),
        projector(|tx, payload| {
            let e: ModelSwitched = decode(payload)?;
            tx.execute(
                "UPDATE session SET model = ?, time_updated = ? WHERE id = ?",
                &[
                    Value::json(&e.model)?,
                    Value::Integer(e.timestamp.millis()),
                    e.session_id.as_str().into(),
                ],
            )?;
            updater::apply(tx, payload)
        }),
    );
    store.project(
        &def("session.next.prompted"),
        projector(|tx, payload| {
            inbox::project_prompted(tx, payload)?;
            updater::apply(tx, payload)
        }),
    );
    store.project(
        &def("session.next.prompt.admitted"),
        projector(inbox::project_admitted),
    );
    for name in [
        "session.next.context.updated",
        "session.next.synthetic",
        "session.next.shell.started",
        "session.next.shell.ended",
        "session.next.step.started",
        "session.next.step.ended",
        "session.next.step.failed",
        "session.next.text.started",
        "session.next.text.ended",
        "session.next.tool.input.started",
        "session.next.tool.input.ended",
        "session.next.tool.called",
        "session.next.tool.progress",
        "session.next.tool.success",
        "session.next.tool.failed",
        "session.next.reasoning.started",
        "session.next.reasoning.ended",
        "session.next.compaction.ended",
    ] {
        store.project(&def(name), projector(updater::apply));
    }
    store.project(
        &definition_of::<RevertStaged>(),
        projector(|tx, payload| {
            let e: RevertStaged = decode(payload)?;
            tx.execute(
                "UPDATE session SET revert = ?, time_updated = ? WHERE id = ?",
                &[
                    Value::json(&e.revert)?,
                    Value::Integer(e.timestamp.millis()),
                    e.session_id.as_str().into(),
                ],
            )?;
            Ok(())
        }),
    );
    store.project(
        &def("session.next.revert.cleared"),
        projector(|tx, payload| {
            tx.execute(
                "UPDATE session SET revert = NULL, time_updated = ? WHERE id = ?",
                &[
                    Value::Integer(timestamp(payload)?),
                    session_id(payload)?.into(),
                ],
            )?;
            Ok(())
        }),
    );
    store.project(
        &definition_of::<RevertCommitted>(),
        projector(project_revert_committed),
    );
}

/// Latest durable definition of a Session event type from the schema manifest.
fn def(name: &str) -> Definition {
    DURABLE_DEFINITIONS
        .iter()
        .rev()
        .find(|candidate| candidate.r#type == name)
        .map(definition)
        .unwrap_or_else(|| panic!("unknown durable Session event {name}"))
}

fn projector(
    run: impl Fn(&dyn Sql, &Payload) -> Result<(), ProjectionError> + Send + Sync + 'static,
) -> Projector {
    Arc::new(move |tx: &dyn Sql, payload: &Payload| {
        run(tx, payload).map_err(|error| Box::new(error) as opencode_event_api::BoxError)
    })
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    payload: &Payload,
) -> Result<T, ProjectionError> {
    serde_json::from_value(payload.data.clone()).map_err(|error| ProjectionError::Decode {
        event_type: payload.event_type.clone(),
        message: error.to_string(),
    })
}

pub(crate) fn session_id(payload: &Payload) -> Result<String, ProjectionError> {
    payload
        .data
        .get("sessionID")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProjectionError::Decode {
            event_type: payload.event_type.clone(),
            message: "missing sessionID".into(),
        })
}

fn timestamp(payload: &Payload) -> Result<i64, ProjectionError> {
    payload
        .data
        .get("timestamp")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value.trunc() as i64)
        .ok_or_else(|| ProjectionError::Decode {
            event_type: payload.event_type.clone(),
            message: "missing timestamp".into(),
        })
}

fn project_moved(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let e: Moved = decode(payload)?;
    tx.execute(
        "UPDATE session SET directory = ?, path = ?, workspace_id = ?, time_updated = ? WHERE id = ?",
        &[
            e.location.directory.as_str().into(),
            e.subdirectory.as_ref().map(|path| path.as_str().to_owned()).into(),
            e.location.workspace_id.as_ref().map(|id| id.as_str().to_owned()).into(),
            Value::Integer(e.timestamp.millis()),
            e.session_id.as_str().into(),
        ],
    )?;
    Ok(())
}

fn project_revert_committed(tx: &dyn Sql, payload: &Payload) -> Result<(), ProjectionError> {
    let e: RevertCommitted = decode(payload)?;
    let session = e.session_id.as_str();
    let boundary = tx
        .query_row(
            "SELECT seq FROM session_message WHERE session_id = ? AND id = ?",
            &[session.into(), e.message_id.as_str().into()],
        )?
        .ok_or_else(|| ProjectionError::RevertBoundaryMissing(e.message_id.to_string()))?
        .get::<i64>("seq")?;
    tx.execute(
        "DELETE FROM session_message WHERE session_id = ? AND seq > ?",
        &[session.into(), Value::Integer(boundary)],
    )?;
    tx.execute(
        "DELETE FROM session_input WHERE session_id = ? AND (admitted_seq > ? OR promoted_seq > ?)",
        &[
            session.into(),
            Value::Integer(boundary),
            Value::Integer(boundary),
        ],
    )?;
    tx.execute(
        "UPDATE session SET revert = NULL, time_updated = ? WHERE id = ?",
        &[Value::Integer(e.timestamp.millis()), session.into()],
    )?;
    Ok(())
}
