//! Purpose: verify ordered todo replacement through the real SQLite adapter and a fixture event probe.
//! Scope: migrated temporary SQLite files. EventStore is fixture-only until phase 02 delivers its adapter.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/todo.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::StreamExt
//! |-- futures::stream
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- stream::once
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests call Todos, not copied replacement logic. Publication probes read committed rows
//! through a separate database handle, and a real SQLite trigger injects a partial-write failure.
//! Contract tests: session-runtime/tests/todo.rs

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{StreamExt, stream};
use opencode_event_api::{
    AggregatePage, Definition, EventError, EventStore, EventStream, Listener, Payload, Projector,
    PublishOptions, ReplayOptions, SerializedEvent, Unsubscribe,
};
use opencode_session_api::{SessionError, SessionID};
use opencode_session_runtime::todo::{TodoInfo, Todos};
use opencode_storage_api::{Database, DatabaseExt, StorageError, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::json;

#[derive(Debug)]
struct Observation {
    payload: Payload,
    committed: Vec<TodoInfo>,
}

// Fixture-only: records ephemeral publish calls, observes committed rows, and injects event failures.
// It is deliberately not an event-store implementation and rejects every unrelated operation.
struct FixtureEvents {
    observer: SqliteDatabase,
    observations: Mutex<Vec<Observation>>,
    failure: AtomicU8,
}

fn unsupported() -> EventError {
    EventError::invalid(
        "fixture",
        "NotYetPorted: fixture supports only ephemeral publish",
    )
}

#[async_trait]
impl EventStore for FixtureEvents {
    async fn publish(
        &self,
        definition: &Definition,
        data: serde_json::Value,
        options: PublishOptions,
    ) -> Result<Payload, EventError> {
        assert_eq!(definition.event_type, "todo.updated");
        assert!(definition.durable.is_none());
        assert!(options.commit.is_none());
        let session = data["sessionID"].as_str().unwrap().to_owned();
        let committed = self
            .observer
            .read(move |tx| {
                tx.query(
                "SELECT content, status, priority FROM todo WHERE session_id = ? ORDER BY position",
                &[session.into()],
            )?.iter().map(|row| Ok(TodoInfo {
                content: row.get("content")?,
                status: row.get("status")?,
                priority: row.get("priority")?,
            })).collect::<Result<Vec<_>, StorageError>>()
            })
            .await
            .unwrap();
        let payload = Payload {
            id: "evt_fixture".into(),
            metadata: options.metadata,
            event_type: definition.event_type.into(),
            durable: None,
            location: options.location,
            data,
        };
        self.observations.lock().unwrap().push(Observation {
            payload: payload.clone(),
            committed,
        });
        match self.failure.load(Ordering::SeqCst) {
            1 => Err(EventError::Storage {
                message: "fixture event failure".into(),
                retryable: true,
                source: None,
            }),
            2 => Err(EventError::Cancelled),
            _ => Ok(payload),
        }
    }

    fn project(&self, _: &Definition, _: Projector) {
        panic!("fixture-only event probe cannot register projectors")
    }
    fn subscribe(&self, _: &Definition) -> EventStream {
        stream::once(async { Err(unsupported()) }).boxed()
    }
    fn all(&self) -> EventStream {
        stream::once(async { Err(unsupported()) }).boxed()
    }
    fn all_bounded(&self, _: usize) -> EventStream {
        stream::once(async { Err(unsupported()) }).boxed()
    }
    fn durable(&self, _: &str, _: Option<i64>) -> EventStream {
        stream::once(async { Err(unsupported()) }).boxed()
    }
    async fn read_aggregate(
        &self,
        _: &str,
        _: Option<i64>,
        _: usize,
        _: &[String],
    ) -> Result<AggregatePage, EventError> {
        Err(unsupported())
    }
    async fn latest_sequence(&self, _: &str) -> Result<i64, EventError> {
        Err(unsupported())
    }
    fn listen(&self, _: Listener) -> Unsubscribe {
        panic!("fixture-only event probe cannot register listeners")
    }
    async fn replay(&self, _: SerializedEvent, _: ReplayOptions) -> Result<(), EventError> {
        Err(unsupported())
    }
    async fn replay_all(
        &self,
        _: Vec<SerializedEvent>,
        _: ReplayOptions,
    ) -> Result<Option<String>, EventError> {
        Err(unsupported())
    }
    async fn remove(&self, _: &str) -> Result<(), EventError> {
        Err(unsupported())
    }
    async fn claim(&self, _: &str, _: &str) -> Result<(), EventError> {
        Err(unsupported())
    }
}

struct Fixture {
    // Keep the directory alive until both database handles have closed.
    _directory: tempfile::TempDir,
    db: SqliteDatabase,
    events: Arc<FixtureEvents>,
    todos: Todos,
    session: SessionID,
    other: SessionID,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("todos.db");
        let db = SqliteDatabase::open(&path).await.unwrap();
        db.transaction(TxOptions::default(), |tx| {
            tx.execute("INSERT INTO project (id, worktree, time_created, time_updated, sandboxes) VALUES ('project_fixture', '/fixture', 1, 1, '[]')", &[])?;
            ["ses_fixture", "ses_other"].iter().try_for_each(|id| {
                tx.execute("INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?, 'project_fixture', 'fixture', '/fixture', 'fixture', '0', 1, 1)", &[Value::from(*id)]).map(|_| ())
            })
        }).await.unwrap();
        let events = Arc::new(FixtureEvents {
            observer: SqliteDatabase::open(&path).await.unwrap(),
            observations: Mutex::new(Vec::new()),
            failure: AtomicU8::new(0),
        });
        let todos = Todos::new(Arc::new(db.clone()), events.clone());
        Self {
            _directory: directory,
            db,
            events,
            todos,
            session: SessionID::new("ses_fixture").unwrap(),
            other: SessionID::new("ses_other").unwrap(),
        }
    }

    async fn close(self) {
        self.events.observer.close().await.unwrap();
        self.db.close().await.unwrap();
    }
}

fn todo(content: &str, status: &str, priority: &str) -> TodoInfo {
    TodoInfo {
        content: content.into(),
        status: status.into(),
        priority: priority.into(),
    }
}

#[tokio::test]
async fn live_sqlite_reads_explicit_position_order() {
    let fixture = Fixture::new().await;
    fixture.db.transaction(TxOptions::default(), |tx| {
        tx.execute_batch("PRAGMA reverse_unordered_selects = ON;")?;
        [7, 2, 4].iter().try_for_each(|position| {
            tx.execute(
                "INSERT INTO todo (session_id, content, status, priority, position, time_created, time_updated) VALUES ('ses_fixture', ?, 'pending', 'high', ?, 1, 1)",
                &[Value::from(position.to_string()), Value::Integer(*position)],
            ).map(|_| ())
        })
    }).await.unwrap();
    assert_eq!(
        fixture.todos.get(&fixture.session).await.unwrap(),
        vec![
            todo("2", "pending", "high"),
            todo("4", "pending", "high"),
            todo("7", "pending", "high"),
        ]
    );
    assert!(fixture.events.observations.lock().unwrap().is_empty());
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_ordered_replacement_is_committed_and_wire_exact() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .todos
            .get(&fixture.session)
            .await
            .unwrap()
            .is_empty()
    );
    let other = vec![todo("other", "pending", "low")];
    fixture
        .todos
        .update(&fixture.other, other.clone())
        .await
        .unwrap();
    fixture
        .todos
        .update(&fixture.session, vec![todo("old", "completed", "high")])
        .await
        .unwrap();
    // Strings are unrestricted in the TypeScript schema, including empty and custom values.
    let replacement = vec![
        todo("second 🦀", "custom", "urgent"),
        todo("", "", ""),
        todo("second 🦀", "pending", "low"),
    ];
    fixture
        .todos
        .update(&fixture.session, replacement.clone())
        .await
        .unwrap();
    assert_eq!(
        fixture.todos.get(&fixture.session).await.unwrap(),
        replacement
    );
    assert_eq!(fixture.todos.get(&fixture.other).await.unwrap(), other);
    let rows = fixture.db.read(|tx| tx.query("SELECT position, time_created, time_updated FROM todo WHERE session_id = 'ses_fixture' ORDER BY position", &[])).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.get::<i64>("position").unwrap())
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(
        rows.iter()
            .all(|row| row.get::<i64>("time_created").unwrap() > 0
                && row.get::<i64>("time_created").unwrap()
                    == row.get::<i64>("time_updated").unwrap())
    );
    {
        let observations = fixture.events.observations.lock().unwrap();
        assert_eq!(observations.len(), 3);
        let event = &observations[2];
        assert_eq!(event.committed, replacement);
        assert_eq!(
            event.payload.data,
            json!({"sessionID": "ses_fixture", "todos": replacement})
        );
        assert_eq!(event.payload.event_type, "todo.updated");
        assert!(event.payload.durable.is_none());
    }
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_empty_replacement_deletes_only_requested_session_and_still_publishes() {
    let fixture = Fixture::new().await;
    let original = vec![todo("keep", "pending", "medium")];
    fixture
        .todos
        .update(&fixture.session, original.clone())
        .await
        .unwrap();
    fixture
        .todos
        .update(&fixture.other, original.clone())
        .await
        .unwrap();
    fixture
        .todos
        .update(&fixture.session, Vec::new())
        .await
        .unwrap();
    fixture
        .todos
        .update(&fixture.session, Vec::new())
        .await
        .unwrap();
    assert!(
        fixture
            .todos
            .get(&fixture.session)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.todos.get(&fixture.other).await.unwrap(), original);
    {
        let observations = fixture.events.observations.lock().unwrap();
        assert_eq!(observations.len(), 4);
        assert_eq!(
            observations[3].payload.data,
            json!({"sessionID": "ses_fixture", "todos": []})
        );
        assert!(observations[2].committed.is_empty());
        assert!(observations[3].committed.is_empty());
    }
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_partial_insert_failure_restores_deleted_rows_without_publication() {
    let fixture = Fixture::new().await;
    let original = vec![todo("original", "pending", "high")];
    fixture
        .todos
        .update(&fixture.session, original.clone())
        .await
        .unwrap();
    fixture.db.transaction(TxOptions::default(), |tx| {
        tx.execute_batch("CREATE TRIGGER todo_fixture_failure BEFORE INSERT ON todo WHEN NEW.content = 'fail' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;")
    }).await.unwrap();
    let error = fixture
        .todos
        .update(
            &fixture.session,
            vec![
                todo("first inserted", "pending", "high"),
                todo("fail", "pending", "low"),
            ],
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::Storage {
            retryable: false,
            ..
        }
    ));
    assert_eq!(fixture.todos.get(&fixture.session).await.unwrap(), original);
    assert_eq!(fixture.events.observations.lock().unwrap().len(), 1);
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_missing_session_foreign_key_failure_emits_nothing() {
    let fixture = Fixture::new().await;
    let missing = SessionID::new("ses_missing").unwrap();
    assert!(fixture.todos.get(&missing).await.unwrap().is_empty());
    let error = fixture
        .todos
        .update(&missing, vec![todo("x", "pending", "high")])
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::Storage {
            retryable: false,
            ..
        }
    ));
    assert!(fixture.events.observations.lock().unwrap().is_empty());
    // The baseline does not validate Session existence on an empty replacement.
    fixture.todos.update(&missing, Vec::new()).await.unwrap();
    assert_eq!(fixture.events.observations.lock().unwrap().len(), 1);
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_publish_failure_preserves_committed_rows_and_retryability() {
    let fixture = Fixture::new().await;
    fixture.events.failure.store(1, Ordering::SeqCst);
    let replacement = vec![todo("committed before error", "pending", "high")];
    let error = fixture
        .todos
        .update(&fixture.session, replacement.clone())
        .await
        .unwrap_err();
    assert!(error.is_retryable());
    assert_eq!(
        fixture.todos.get(&fixture.session).await.unwrap(),
        replacement
    );
    assert_eq!(
        fixture.events.observations.lock().unwrap()[0].committed,
        replacement
    );
    fixture.events.failure.store(2, Ordering::SeqCst);
    assert!(matches!(
        fixture.todos.update(&fixture.session, Vec::new()).await,
        Err(SessionError::Cancelled)
    ));
    assert!(
        fixture
            .todos
            .get(&fixture.session)
            .await
            .unwrap()
            .is_empty()
    );
    fixture.close().await;
}

#[tokio::test]
async fn fixture_event_closed_database_failure_does_not_publish() {
    let fixture = Fixture::new().await;
    fixture.db.close().await.unwrap();
    let error = fixture
        .todos
        .update(&fixture.session, Vec::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::Storage {
            retryable: false,
            ..
        }
    ));
    assert!(fixture.todos.get(&fixture.session).await.is_err());
    assert!(fixture.events.observations.lock().unwrap().is_empty());
    fixture.events.observer.close().await.unwrap();
}
