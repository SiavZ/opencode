//! Purpose: compose persisted V1 Session operations and the real global event bridge for HTTP.
//! Scope: process-scoped adapters over ProjectStore, LegacyStore, RunState and EventStore.
//! Uses:
//! rust/crates/
//! |-- locations/src/project_store.rs
//! |-- server/src/services/legacy.rs
//! |-- server/src/services/streams.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/service/util.rs
//! External:
//! |-- futures::StreamExt
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: creation commits one real placed event through the existing projector. Reads use
//! persisted rows and SQL filtering, with no fabricated sessions or duplicate event publication.
//! Contract tests: runtime/tests/legacy.rs

use std::sync::Arc;

use futures::StreamExt;
use opencode_event_api::{Definition, EventStore, LocationRef, PublishOptions};
use opencode_locations::ProjectResolver;
use opencode_locations::project_store::ProjectStore;
use opencode_schema::ids::SessionID;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyError, LegacySessions};
use opencode_server::services::streams::{GlobalBus, GlobalEvent};
use opencode_session_runtime::legacy::run_state::RunState;
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::legacy::types::now_millis;
use opencode_session_runtime::service::{iso, relative, slug};
use opencode_storage_api::{DatabaseExt, Value as StorageValue};
use serde_json::{Map, Value, json};

pub struct LegacyAdapter {
    store: LegacyStore,
    projects: ProjectStore,
    version: String,
    run_state: RunState,
}

impl LegacyAdapter {
    pub fn new(
        store: LegacyStore,
        projects: Arc<dyn ProjectResolver>,
        version: String,
        run_state: RunState,
    ) -> Self {
        Self {
            projects: ProjectStore::new(store.events().database(), projects),
            store,
            version,
            run_state,
        }
    }

    async fn create(
        &self,
        context: &RouteContext,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let input = payload.unwrap_or_else(|| json!({}));
        let input = input
            .as_object()
            .ok_or_else(|| LegacyError::BadRequest("Invalid session payload".into()))?;
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?;
        let id = SessionID::create().to_string();
        let now = now_millis();
        let title = input
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    if input.contains_key("parentID") {
                        "Child session - "
                    } else {
                        "New session - "
                    },
                    iso(now)
                )
            });
        let mut info = json!({
            "id":id,"slug":slug(),"version":self.version,"projectID":project.project.id.as_str(),
            "directory":context.directory,"path":relative(&project.project.worktree, &context.directory),
            "title":title,"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
            "time":{"created":now,"updated":now}
        });
        let object = info.as_object_mut().expect("literal object");
        for key in ["parentID", "agent", "model", "metadata", "permission"] {
            if let Some(value) = input.get(key) {
                object.insert(key.into(), value.clone());
            }
        }
        let workspace = input
            .get("workspaceID")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| context.workspace_id.clone());
        if let Some(workspace) = &workspace {
            object.insert("workspaceID".into(), json!(workspace));
        }
        self.store
            .events()
            .store()
            .publish(
                &Definition::durable("session.created", 1, "sessionID"),
                json!({"sessionID":id,"info":info}),
                PublishOptions {
                    location: Some(LocationRef {
                        directory: context.directory.clone(),
                        workspace_id: workspace,
                    }),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(internal)?;
        Ok(info)
    }

    async fn list(
        &self,
        context: &RouteContext,
        query: &Map<String, Value>,
    ) -> Result<Value, LegacyError> {
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?;
        let mut conditions = vec!["project_id = ?".to_owned()];
        let mut params = vec![StorageValue::from(project.project.id.as_str())];
        let directory = (query.get("scope").and_then(Value::as_str) != Some("project"))
            .then(|| {
                query
                    .get("directory")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|_| context.directory.clone())
            })
            .flatten();
        if let Some(path) = query.get("path").and_then(Value::as_str) {
            if !path.is_empty() {
                params.extend([
                    StorageValue::from(path),
                    StorageValue::from(format!("{path}/%")),
                ]);
                if let Some(directory) = &directory {
                    conditions.push(
                        "(path = ? OR path LIKE ? OR (path IS NULL AND directory = ?))".into(),
                    );
                    params.push(directory.into());
                } else {
                    conditions.push("(path = ? OR path LIKE ?)".into());
                }
            }
        } else if let Some(directory) = directory {
            conditions.push("directory = ?".into());
            params.push(directory.into());
        }
        if query.get("roots").and_then(Value::as_bool) == Some(true) {
            conditions.push("parent_id IS NULL".into());
        }
        if let Some(start) = query
            .get("start")
            .and_then(Value::as_f64)
            .filter(|value| *value != 0.0)
        {
            conditions.push("time_updated >= ?".into());
            params.push(start.into());
        }
        if let Some(search) = query
            .get("search")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            conditions.push("title LIKE ?".into());
            params.push(format!("%{search}%").into());
        }
        params.push(
            query
                .get("limit")
                .and_then(Value::as_f64)
                .unwrap_or(100.0)
                .into(),
        );
        let sql = format!(
            "SELECT id FROM session WHERE {} ORDER BY time_updated DESC LIMIT ?",
            conditions.join(" AND ")
        );
        let rows = self
            .store
            .events()
            .database()
            .read(move |tx| tx.query(&sql, &params))
            .await
            .map_err(internal)?;
        let mut sessions = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.get("id").map_err(internal)?;
            sessions.push(Value::Object(self.store.get(&id).await.map_err(map_error)?));
        }
        Ok(numbers(Value::Array(sessions)))
    }
}

#[async_trait::async_trait]
impl LegacySessions for LegacyAdapter {
    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let id = || {
            params
                .get("sessionID")
                .and_then(Value::as_str)
                .ok_or_else(|| LegacyError::BadRequest("Missing sessionID".into()))
        };
        match operation {
            "session.create" => self.create(context, payload).await,
            "session.list" => self.list(context, query).await,
            "session.status" => Ok(json!(self.run_state.status().list())),
            "session.get" => self
                .store
                .get(id()?)
                .await
                .map(Value::Object)
                .map(numbers)
                .map_err(map_error),
            "session.messages" => {
                let before = query
                    .get("before")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                if before.is_some() && !query.contains_key("limit") {
                    return Err(LegacyError::BadRequest("before requires limit".into()));
                }
                if before.is_some() {
                    return Err(LegacyError::Unavailable(
                        "session.messages.pagination".into(),
                    ));
                }
                let messages = self.store.messages(id()?).await.map_err(map_error)?;
                let limit = query.get("limit").and_then(Value::as_u64).unwrap_or(0);
                if limit != 0 && messages.len() as u64 > limit {
                    return Err(LegacyError::Unavailable(
                        "session.messages.pagination".into(),
                    ));
                }
                serde_json::to_value(messages)
                    .map(numbers)
                    .map_err(internal)
            }
            "session.message" => {
                let message = params
                    .get("messageID")
                    .and_then(Value::as_str)
                    .ok_or_else(|| LegacyError::BadRequest("Missing messageID".into()))?;
                self.store
                    .messages(id()?)
                    .await
                    .map_err(map_error)?
                    .into_iter()
                    .find(|row| row.id() == message)
                    .ok_or_else(|| LegacyError::NotFound(format!("Message not found: {message}")))
                    .and_then(|row| serde_json::to_value(row).map(numbers).map_err(internal))
            }
            "session.abort" => {
                self.run_state.cancel(id()?).await;
                Ok(json!(true))
            }
            _ => Err(LegacyError::Unavailable(operation.into())),
        }
    }
}

fn internal(error: impl std::fmt::Display) -> LegacyError {
    LegacyError::Internal(error.to_string())
}

fn map_error(error: opencode_session_runtime::legacy::types::LegacyError) -> LegacyError {
    match error {
        opencode_session_runtime::legacy::types::LegacyError::NotFound(id) => {
            LegacyError::NotFound(format!("Session not found: {id}"))
        }
        opencode_session_runtime::legacy::types::LegacyError::NotYetPorted(name) => {
            LegacyError::Unavailable(name.into())
        }
        other => internal(other),
    }
}

// LegacyStore exposes SQLite REAL columns as f64. JSON.stringify encodes integral values as integers.
fn numbers(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(numbers).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, numbers(value)))
                .collect(),
        ),
        Value::Number(ref number) if number.is_f64() => number
            .as_f64()
            .filter(|value| value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0)
            .map(|value| json!(value as i64))
            .unwrap_or(value),
        _ => value,
    }
}

pub struct LegacyEventBus {
    events: Arc<dyn EventStore>,
}

impl LegacyEventBus {
    pub fn new(events: Arc<dyn EventStore>) -> Self {
        Self { events }
    }
}

impl GlobalBus for LegacyEventBus {
    fn subscribe(&self) -> futures::stream::BoxStream<'static, GlobalEvent> {
        self.events
            .all()
            .filter_map(|event| async move {
                let event = event.ok()?;
                let payload =
                    json!({"id":event.id,"type":event.event_type,"properties":event.data});
                Some(match event.location {
                    Some(location) => {
                        let mut event = json!({"directory":location.directory,"payload":payload});
                        if let Some(workspace) = location.workspace_id {
                            event["workspace"] = json!(workspace);
                        }
                        event
                    }
                    None => json!({"payload":payload}),
                })
            })
            .boxed()
    }
}
