//! Purpose: `SessionService` implementation (packages/core/src/session.ts `SessionV2` layer) composing
//! durable admission, projected reads, event publication and process-global execution.
//! Scope: process-global service; Location-scoped work (project resolution, revert snapshots) goes
//! through ports resolved per Session location.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/event.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/location.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/src/info.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/service/util.rs
//! |-- session-runtime/src/store.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::StreamExt::filter
//! |-- location::Ref
//! |-- serde_json::json
//! |-- serde_json::to_value
//! `-- tokio::spawn
//! Used by:
//! rust/crates/
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/legacy.rs
//! |-- runtime/src/process.rs
//! |-- session-runtime/src/children/store.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! |-- session-runtime/tests/admission_projection.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: `prompt` commits admission before the advisory wake, wakes only when `resume` is not
//! `Some(false)`, and rejects non-equivalent reuse with `PromptConflict`. `create` adopts an existing
//! Session ID, including when a concurrent create wins the projection race.
//! Contract tests: session-runtime/tests/admission.rs, session-runtime/tests/admission_historical.rs

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::{EventError, EventStream, PublishOptions};
use opencode_schema::event::EventData;
use opencode_schema::session_event::{AgentSwitched, DURABLE_DEFINITIONS, ModelSwitched};
use opencode_schema::values::{AbsolutePath, DateTimeUtc};
use opencode_schema::{ProjectID, location};
use opencode_session_api::admission::{PromptRequest, equivalent};
use opencode_session_api::error::UnavailableOperation;
use opencode_session_api::ids::same_model_selection;
use opencode_session_api::info::{RevertState, SessionInfo};
use opencode_session_api::operations::{
    CreateRequest, HistoryPage, ListRequest, MessagesRequest, RevertStageRequest, SessionExecution,
    SessionService,
};
use opencode_session_api::{
    Admitted, Delivery, Message, MessageID, ModelRef, SessionError, SessionID, SessionResult,
};
use opencode_storage_api::{DatabaseExt, Sql, TxOptions, Value};
use serde_json::json;

use crate::admission::{Admission, admit, resolve_prompt};
use crate::projection::{
    ProjectionError, SessionEvents, event_error, publish_session_event, storage_error,
};
use crate::revert::Revert;
use crate::store::SessionStore;

mod lifecycle;
mod util;

pub use util::{iso, relative, slug};

/// `ProjectV2.resolve` result used by `create`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProject {
    pub id: ProjectID,
    pub directory: AbsolutePath,
    /// `vcs.type` (`"git"`) when the directory is inside a repository.
    pub vcs: Option<String>,
}

/// Location-scoped ports the Session service needs (`ProjectV2.Service`, `LocationServiceMap`).
#[async_trait]
pub trait SessionLocations: Send + Sync {
    /// `ProjectV2.resolve(directory)`.
    async fn resolve_project(&self, directory: &AbsolutePath) -> SessionResult<ResolvedProject>;
    /// Revert operations bound to the Location's snapshot service.
    fn revert(&self, location: &location::Ref) -> SessionResult<Revert>;
}

/// `InstallationVersion` for builds without an injected version.
pub const LOCAL_VERSION: &str = "local";

/// Process-global `SessionV2` service.
#[derive(Clone)]
pub struct LocalSessionService {
    events: Arc<dyn SessionEvents>,
    store: SessionStore,
    execution: Arc<dyn SessionExecution>,
    locations: Arc<dyn SessionLocations>,
    version: String,
}

impl LocalSessionService {
    pub fn new(
        events: Arc<dyn SessionEvents>,
        execution: Arc<dyn SessionExecution>,
        locations: Arc<dyn SessionLocations>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            store: SessionStore::new(events.database()),
            events,
            execution,
            locations,
            version: version.into(),
        }
    }

    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// `SessionV2.events`: durable Session events after `after`, then live events.
    pub async fn events(
        &self,
        session: &SessionID,
        after: Option<i64>,
    ) -> SessionResult<EventStream> {
        self.get(session).await?;
        let durable: BTreeSet<&str> = DURABLE_DEFINITIONS.iter().map(|d| d.r#type).collect();
        let stream = self.events.store().durable(session.as_str(), after);
        Ok(Box::pin(futures::StreamExt::filter(stream, move |event| {
            let keep = event.as_ref().map_or(true, |payload| {
                durable.contains(payload.event_type.as_str())
            });
            std::future::ready(keep)
        })))
    }

    async fn require(&self, session: &SessionID) -> SessionResult<SessionInfo> {
        self.store
            .get(session)
            .await?
            .ok_or_else(|| SessionError::NotFound {
                session_id: session.clone(),
            })
    }

    async fn publish_created(
        &self,
        request: &CreateRequest,
        id: &SessionID,
    ) -> SessionResult<Option<SessionInfo>> {
        let project = self
            .locations
            .resolve_project(&request.location.directory)
            .await?;
        let insert = project.clone();
        self.events
            .database()
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(
                    "INSERT INTO project (id, worktree, vcs, sandboxes, time_created, time_updated) VALUES (?, ?, ?, '[]', ?, ?) ON CONFLICT DO NOTHING",
                    &[
                        insert.id.as_str().into(),
                        insert.directory.as_str().into(),
                        insert.vcs.clone().into(),
                        Value::Integer(DateTimeUtc::now().millis()),
                        Value::Integer(DateTimeUtc::now().millis()),
                    ],
                )
            })
            .await
            .map_err(storage_error)?;
        let now = DateTimeUtc::now().millis();
        let mut info = json!({
            "id": id,
            "slug": slug(),
            "version": self.version,
            "projectID": project.id,
            "directory": request.location.directory,
            "path": relative(project.directory.as_str(), request.location.directory.as_str()),
            "title": format!("New session - {}", iso(now)),
            "cost": 0,
            "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
            "time": {"created": now, "updated": now},
        });
        if let Some(workspace) = &request.location.workspace_id {
            info["workspaceID"] = json!(workspace);
        }
        if let Some(agent) = &request.agent {
            info["agent"] = json!(agent);
        }
        if let Some(model) = &request.model {
            info["model"] = json!(model);
        }
        let options = PublishOptions {
            location: Some(opencode_event_api::LocationRef {
                directory: request.location.directory.as_str().into(),
                workspace_id: request
                    .location
                    .workspace_id
                    .as_ref()
                    .map(|id| id.as_str().into()),
            }),
            ..PublishOptions::default()
        };
        let created = self
            .events
            .store()
            .publish(
                &opencode_event_api::Definition::durable("session.created", 1, "sessionID"),
                json!({"sessionID": id, "info": info}),
                options,
            )
            .await;
        match created {
            Ok(_) => Ok(None),
            Err(error) if is_already_projected(&error) => {
                // Concurrent creation lost the projection race. The existing Session identity wins.
                self.store
                    .get(id)
                    .await?
                    .map(Some)
                    .ok_or_else(|| event_error(error))
            }
            Err(error) => Err(event_error(error)),
        }
    }

    async fn publish_event<T: EventData>(&self, data: &T) -> SessionResult<()> {
        publish_session_event(self.events.as_ref(), data)
            .await
            .map(|_| ())
    }
}

fn is_already_projected(error: &EventError) -> bool {
    matches!(
        error.projector_source::<ProjectionError>(),
        Some(ProjectionError::SessionAlreadyProjected)
    )
}

#[async_trait]
impl SessionService for LocalSessionService {
    async fn rename(&self, session_id: &SessionID, title: &str) -> SessionResult<()> {
        self.rename_host(session_id, title).await
    }

    async fn delete(&self, session_id: &SessionID) -> SessionResult<()> {
        self.delete_host(session_id).await
    }

    async fn fork(
        &self,
        session_id: &SessionID,
        message_id: Option<&MessageID>,
    ) -> SessionResult<SessionInfo> {
        self.fork_host(session_id, message_id).await
    }

    async fn list(&self, request: ListRequest) -> SessionResult<Vec<SessionInfo>> {
        self.store.list(request).await
    }

    async fn create(&self, request: CreateRequest) -> SessionResult<SessionInfo> {
        let id = request.id.clone().unwrap_or_else(SessionID::create);
        if let Some(recorded) = self.store.get(&id).await? {
            return Ok(recorded);
        }
        if let Some(existing) = self.publish_created(&request, &id).await? {
            return Ok(existing);
        }
        self.require(&id).await
    }

    async fn get(&self, session_id: &SessionID) -> SessionResult<SessionInfo> {
        self.require(session_id).await
    }

    async fn messages(&self, request: MessagesRequest) -> SessionResult<Vec<Message>> {
        self.require(&request.session_id).await?;
        self.store.messages(request).await
    }

    async fn message(
        &self,
        session_id: &SessionID,
        message_id: &MessageID,
    ) -> SessionResult<Option<Message>> {
        Ok(self
            .store
            .message(message_id)
            .await?
            .filter(|(owner, _)| owner == session_id)
            .map(|(_, message)| message))
    }

    async fn context(&self, session_id: &SessionID) -> SessionResult<Vec<Message>> {
        self.require(session_id).await?;
        self.store.context(session_id).await
    }

    async fn history(
        &self,
        session_id: &SessionID,
        after: Option<i64>,
        limit: usize,
    ) -> SessionResult<HistoryPage> {
        self.require(session_id).await?;
        let types: Vec<String> = DURABLE_DEFINITIONS
            .iter()
            .filter_map(|definition| {
                definition
                    .durable
                    .map(|durable| format!("{}.{}", definition.r#type, durable.version))
            })
            .collect();
        let page = self
            .events
            .store()
            .read_aggregate(session_id.as_str(), after, limit, &types)
            .await
            .map_err(event_error)?;
        let events = page
            .events
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SessionError::Storage {
                message: error.to_string(),
                retryable: false,
            })?;
        Ok(HistoryPage {
            events,
            has_more: page.has_more,
        })
    }

    async fn prompt(&self, request: PromptRequest) -> SessionResult<Admitted> {
        self.require(&request.session_id).await?;
        let prompt = resolve_prompt(request.prompt);
        let id = request.id.unwrap_or_else(MessageID::create);
        let delivery = request.delivery.unwrap_or(Delivery::Steer);
        let admitted = admit(
            self.events.as_ref(),
            Admission {
                id: id.clone(),
                session_id: request.session_id.clone(),
                prompt: prompt.clone(),
                delivery,
            },
        )
        .await?;
        if !equivalent(&admitted, &request.session_id, &prompt, delivery) {
            return Err(SessionError::PromptConflict {
                session_id: request.session_id,
                message_id: id,
            });
        }
        if request.resume != Some(false) {
            self.execution.wake(&admitted.session_id).await;
        }
        Ok(admitted)
    }

    async fn switch_agent(&self, session_id: &SessionID, agent: &str) -> SessionResult<()> {
        self.require(session_id).await?;
        self.publish_event(&AgentSwitched {
            timestamp: DateTimeUtc::now(),
            session_id: session_id.clone(),
            message_id: MessageID::create(),
            agent: agent.to_owned(),
        })
        .await
    }

    async fn switch_model(&self, session_id: &SessionID, model: ModelRef) -> SessionResult<()> {
        let session = self.require(session_id).await?;
        if session
            .model
            .as_ref()
            .is_some_and(|current| same_model_selection(current, &model))
        {
            return Ok(());
        }
        self.publish_event(&ModelSwitched {
            timestamp: DateTimeUtc::now(),
            session_id: session_id.clone(),
            message_id: MessageID::create(),
            model,
        })
        .await
    }

    async fn shell(&self, _: &SessionID, _: &str) -> SessionResult<()> {
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Shell,
        })
    }

    async fn skill(&self, _: &SessionID, _: &str) -> SessionResult<()> {
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Skill,
        })
    }

    async fn compact(&self, session_id: &SessionID) -> SessionResult<()> {
        self.require(session_id).await?;
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Compact,
        })
    }

    async fn wait(&self, session_id: &SessionID) -> SessionResult<()> {
        self.require(session_id).await?;
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Wait,
        })
    }

    async fn active(&self) -> BTreeSet<SessionID> {
        self.execution.active().await
    }

    async fn resume(&self, session_id: &SessionID) -> SessionResult<()> {
        self.require(session_id).await?;
        self.execution.resume(session_id).await
    }

    async fn interrupt(&self, session_id: &SessionID) {
        // Interrupt runs to completion even if the caller is dropped (`Effect.uninterruptible`).
        let execution = self.execution.clone();
        let session = session_id.clone();
        let handle = tokio::spawn(async move { execution.interrupt(&session).await });
        let _ = handle.await;
    }

    async fn revert_stage(&self, request: RevertStageRequest) -> SessionResult<RevertState> {
        let session = self.require(&request.session_id).await?;
        self.locations
            .revert(&session.location)?
            .stage(&session, &request.message_id, request.files)
            .await
    }

    async fn revert_clear(&self, session_id: &SessionID) -> SessionResult<()> {
        let session = self.require(session_id).await?;
        self.locations
            .revert(&session.location)?
            .clear(&session)
            .await
    }

    async fn revert_commit(&self, session_id: &SessionID) -> SessionResult<()> {
        let session = self.require(session_id).await?;
        self.locations
            .revert(&session.location)?
            .commit(&session)
            .await
    }
}
