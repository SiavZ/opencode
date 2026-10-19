//! Purpose: durable ordered todo replacement plus `todo.updated`, porting
//! packages/core/src/session/todo.ts and the identical legacy packages/opencode/src/session/todo.ts.
//! Scope: one write transaction per update on the shared `todo` table, then an ephemeral event.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/event.rs
//! |-- schema/src/session_aux_event.rs
//! |-- session-api/src/lib.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/src/revert.rs
//! `-- session-runtime/tests/todo.rs
//! Invariants: the delete and the ordered insert commit together (position = array index) before
//! `todo.updated` is published, so subscribers never observe an event for uncommitted rows. An
//! empty list only deletes. Reads order by position ascending.
//! Contract tests: session-runtime/tests/todo.rs

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use opencode_event_api::{Definition, EventError, EventStore, PublishOptions};
use opencode_schema::event::EventData;
use opencode_schema::session_aux_event::{Todo, TodoUpdated};
use opencode_session_api::{SessionError, SessionID};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, TxOptions, Value};

/// `SessionTodo.Info` (`packages/schema/src/session-todo.ts`), owned by opencode-schema.
pub type TodoInfo = Todo;

/// Event facade used by lifecycle services. Same transaction type as the SQLite event store.
pub type Events = Arc<dyn EventStore>;

/// `SessionTodo.Service`. Location-scoped in the baseline, but holds no per-Session state.
#[derive(Clone)]
pub struct Todos {
    db: Arc<dyn Database>,
    events: Events,
}

impl Todos {
    pub fn new(db: Arc<dyn Database>, events: Events) -> Self {
        Self { db, events }
    }

    /// Replace every todo of the Session in order, then publish `todo.updated`.
    pub async fn update(
        &self,
        session_id: &SessionID,
        todos: Vec<TodoInfo>,
    ) -> Result<(), SessionError> {
        let session = session_id.as_str().to_owned();
        let rows = todos.clone();
        self.db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(
                    "DELETE FROM todo WHERE session_id = ?",
                    &[Value::from(&session)],
                )?;
                let now = now_millis();
                rows.iter().enumerate().try_for_each(|(position, todo)| {
                    tx.execute(
                        "INSERT INTO todo (session_id, content, status, priority, position, time_created, time_updated) VALUES (?, ?, ?, ?, ?, ?, ?)",
                        &[
                            Value::from(&session),
                            Value::from(&todo.content),
                            Value::from(&todo.status),
                            Value::from(&todo.priority),
                            Value::Integer(position as i64),
                            Value::Integer(now),
                            Value::Integer(now),
                        ],
                    )
                    .map(|_| ())
                })
            })
            .await
            .map_err(storage_error)?;
        let data = TodoUpdated {
            session_id: session_id.clone(),
            todos,
        };
        publish(&self.events, &data).await
    }

    /// Todos of the Session ordered by position.
    pub async fn get(&self, session_id: &SessionID) -> Result<Vec<TodoInfo>, SessionError> {
        let session = session_id.as_str().to_owned();
        self.db
            .read(move |tx: &dyn Sql| {
                tx.query(
                    "SELECT content, status, priority FROM todo WHERE session_id = ? ORDER BY position ASC",
                    &[Value::from(session)],
                )?
                .iter()
                .map(|row| {
                    Ok(TodoInfo {
                        content: row.get("content")?,
                        status: row.get("status")?,
                        priority: row.get("priority")?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()
            })
            .await
            .map_err(storage_error)
    }
}

/// Publish a schema-typed event through the event store with default options.
pub(crate) async fn publish<T: EventData>(events: &Events, data: &T) -> Result<(), SessionError> {
    let value = serde_json::to_value(data).map_err(|error| SessionError::Storage {
        message: error.to_string(),
        retryable: false,
    })?;
    events
        .publish(&definition::<T>(), value, PublishOptions::default())
        .await
        .map(|_| ())
        .map_err(event_error)
}

/// event-api definition for a schema `EventData` type.
pub(crate) fn definition<T: EventData>() -> Definition {
    match T::DEFINITION.durable {
        Some(durable) => {
            Definition::durable(T::DEFINITION.r#type, durable.version, durable.aggregate)
        }
        None => Definition::ephemeral(T::DEFINITION.r#type),
    }
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

pub(crate) fn storage_error(error: StorageError) -> SessionError {
    if error.is_cancelled() {
        return SessionError::Cancelled;
    }
    SessionError::Storage {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}

pub(crate) fn event_error(error: EventError) -> SessionError {
    if matches!(error, EventError::Cancelled) {
        return SessionError::Cancelled;
    }
    SessionError::Storage {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{event_error, storage_error};
    use opencode_event_api::EventError;
    use opencode_session_api::SessionError;
    use opencode_storage_api::StorageError;

    #[test]
    fn infrastructure_errors_preserve_cancellation_and_retryability() {
        assert!(matches!(
            storage_error(StorageError::Cancelled),
            SessionError::Cancelled
        ));
        assert!(matches!(
            event_error(EventError::Cancelled),
            SessionError::Cancelled
        ));
        assert!(storage_error(StorageError::Busy("fixture contention".into())).is_retryable());
        assert!(storage_error(StorageError::Locked("fixture contention".into())).is_retryable());
        assert!(
            !storage_error(StorageError::ForeignKey("fixture missing parent".into()))
                .is_retryable()
        );
        assert!(!event_error(EventError::Closed).is_retryable());
    }
}
