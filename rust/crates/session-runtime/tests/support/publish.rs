//! Purpose: test support for the LLM event publisher: a fixture recording sink forwarding to the
//! real p02 SqlEventStore, a publisher fixture over migrated SQLite, and LLM event builders.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- llm-api/src/event.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/publish.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/runner_publish.rs
//! `-- session-runtime/tests/runner_publish_tools.rs
//! Invariants: the recorder is a fixture wrapper only: every publish is forwarded to the real
//! store so projectors and sequences run for real.
//! Contract tests: session-runtime/tests/runner_publish*.rs
#![allow(dead_code)]

use super::sql;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_event_api::{
    AggregatePage, Definition, EventError, EventStore, EventStream, Listener, Payload, Projector,
    PublishOptions, ReplayOptions, SerializedEvent, Unsubscribe,
};
use opencode_llm_api::event::LlmEvent;
use opencode_llm_api::message::{ToolContent, ToolOutput, ToolResultValue};
use opencode_schema::provider::ModelRef;
use opencode_session_api::SessionID;
use opencode_session_runtime::projection::{LocalSessionEvents, SessionEvents};
use opencode_session_runtime::runner::publish::{LlmEventPublisher, PublisherInput};
use opencode_storage_api::Database;
use serde_json::{Value, json};

pub const SESSION: &str = "ses_publish";

/// Fixture: records `(versioned type, data)` for every publish, then forwards to the real store.
pub struct Recorder {
    pub inner: Arc<dyn EventStore>,
    pub published: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl EventStore for Recorder {
    async fn publish(
        &self,
        definition: &Definition,
        data: Value,
        options: PublishOptions,
    ) -> Result<Payload, EventError> {
        let name = match &definition.durable {
            Some(durable) => format!("{}.{}", definition.event_type, durable.version),
            None => definition.event_type.to_string(),
        };
        self.published.lock().unwrap().push((name, data.clone()));
        self.inner.publish(definition, data, options).await
    }
    fn project(&self, definition: &Definition, projector: Projector) {
        self.inner.project(definition, projector)
    }
    fn subscribe(&self, definition: &Definition) -> EventStream {
        self.inner.subscribe(definition)
    }
    fn all(&self) -> EventStream {
        self.inner.all()
    }
    fn all_bounded(&self, capacity: usize) -> EventStream {
        self.inner.all_bounded(capacity)
    }
    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream {
        self.inner.durable(aggregate_id, after)
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
        self.inner.listen(listener)
    }
    async fn replay(
        &self,
        event: SerializedEvent,
        options: ReplayOptions,
    ) -> Result<(), EventError> {
        self.inner.replay(event, options).await
    }
    async fn replay_all(
        &self,
        events: Vec<SerializedEvent>,
        options: ReplayOptions,
    ) -> Result<Option<String>, EventError> {
        self.inner.replay_all(events, options).await
    }
    async fn remove(&self, aggregate_id: &str) -> Result<(), EventError> {
        self.inner.remove(aggregate_id).await
    }
    async fn claim(&self, aggregate_id: &str, owner_id: &str) -> Result<(), EventError> {
        self.inner.claim(aggregate_id, owner_id).await
    }
}

pub struct Fixture {
    pub h: sql::Harness,
    pub recorder: Arc<Recorder>,
    pub publisher: LlmEventPublisher,
}

impl Fixture {
    pub async fn new() -> Self {
        let h = sql::open().await;
        h.seed_session(SESSION).await;
        let recorder = Arc::new(Recorder {
            inner: h.store.clone(),
            published: Mutex::new(Vec::new()),
        });
        let db: Arc<dyn Database> = h.db.clone();
        let events: Arc<dyn SessionEvents> =
            Arc::new(LocalSessionEvents::new(db, recorder.clone()));
        let publisher = LlmEventPublisher::new(
            events,
            PublisherInput {
                session_id: SessionID::new(SESSION).unwrap(),
                agent: "build".into(),
                model: serde_json::from_value::<ModelRef>(
                    json!({"id": "model", "providerID": "provider"}),
                )
                .unwrap(),
                snapshot: Some("snap_1".into()),
            },
        );
        Self {
            h,
            recorder,
            publisher,
        }
    }

    pub async fn send(&mut self, event: LlmEvent) {
        self.publisher.publish(&event, &[]).await.unwrap();
    }

    pub fn types(&self) -> Vec<String> {
        self.recorder
            .published
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<Value> {
        self.recorder
            .published
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, data)| data.clone())
    }

    pub async fn messages(&self) -> Vec<Value> {
        self.h
            .query(
                "SELECT type, data FROM session_message WHERE session_id = ? ORDER BY seq",
                vec![SESSION.into()],
            )
            .await
            .iter()
            .map(|row| {
                let mut data: Value =
                    serde_json::from_str(&row.get::<String>("data").unwrap()).unwrap();
                data["type"] = Value::String(row.get::<String>("type").unwrap());
                data
            })
            .collect()
    }
}

pub fn call(id: &str, provider_executed: Option<bool>) -> LlmEvent {
    LlmEvent::ToolCall {
        id: id.into(),
        name: "read".into(),
        input: json!({"path": "pixel.png"}),
        provider_executed,
        provider_metadata: None,
    }
}

pub fn media_result(id: &str, provider_executed: Option<bool>) -> LlmEvent {
    let content = vec![
        ToolContent::Text {
            text: "Image read successfully".into(),
        },
        ToolContent::File {
            uri: "data:image/png;base64,iVBORw0KGgo".into(),
            mime: "image/png".into(),
            name: Some("pixel.png".into()),
        },
    ];
    LlmEvent::ToolResult {
        id: id.into(),
        name: "read".into(),
        result: ToolResultValue::Content(content.clone()),
        output: Some(ToolOutput {
            structured: json!({"type": "media", "mime": "image/png"}),
            content,
        }),
        provider_executed,
        provider_metadata: None,
    }
}

pub fn text_events() -> Vec<LlmEvent> {
    vec![
        LlmEvent::StepStart { index: 0 },
        LlmEvent::TextStart {
            id: "t1".into(),
            provider_metadata: None,
        },
        LlmEvent::TextDelta {
            id: "t1".into(),
            text: "Hel".into(),
            provider_metadata: None,
        },
        LlmEvent::TextDelta {
            id: "t1".into(),
            text: "lo".into(),
            provider_metadata: None,
        },
        LlmEvent::TextEnd {
            id: "t1".into(),
            provider_metadata: None,
        },
    ]
}
