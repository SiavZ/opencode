//! Purpose: compose durable saved grants, persisted Session agent policy and Location permission events.
//! Scope: Location-scoped native adapters over the existing permission domain and SQL repositories.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- permissions/src/lib.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/store.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/repositories/domains.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! `-- none, reserved contract (owner: phase-90, Location composition)
//! Invariants: configured denies override saved grants; missing Sessions fail; events use the shared Location store.
//! Contract tests: runtime/tests/permissions.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::{Definition, LocationRef, PublishOptions};
use opencode_permissions::{
    Effect, PermissionError, PermissionEvent, PermissionEvents, PermissionService, PolicySource,
    Rule, Ruleset, SavedGrant, SavedGrants,
};
use opencode_session_api::SessionID;
use opencode_session_runtime::{agents::Agents, projection::SessionEvents, store::SessionStore};
use opencode_storage_api::{Database, DatabaseExt, TxOptions};
use opencode_storage_sqlite::repositories::permission;
use serde_json::json;

pub struct Permissions {
    pub service: Arc<PermissionService>,
    pub saved: Arc<Saved>,
}

pub fn build(
    project_id: String,
    agents: Agents,
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
) -> Permissions {
    let saved = Arc::new(Saved(events.database()));
    let service = Arc::new(PermissionService::new(
        project_id,
        Arc::new(Policy {
            agents,
            sessions: SessionStore::new(events.database()),
        }),
        saved.clone(),
        Arc::new(Events { events, location }),
    ));
    Permissions { service, saved }
}

pub struct Saved(pub Arc<dyn Database>);

impl Saved {
    pub async fn list(&self, project: Option<&str>) -> Result<Vec<SavedGrant>, PermissionError> {
        let project = project.map(str::to_owned);
        self.0
            .read(move |sql| permission::list(sql, project.as_deref()))
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| SavedGrant {
                        id: row.id,
                        project_id: row.project_id,
                        action: row.action,
                        resource: row.resource,
                    })
                    .collect()
            })
            .map_err(port)
    }

    pub async fn remove(&self, id: &str) -> Result<(), PermissionError> {
        let id = id.to_owned();
        self.0
            .transaction(TxOptions::default(), move |sql| {
                permission::remove(sql, &id)
            })
            .await
            .map_err(port)
    }
}

#[async_trait]
impl SavedGrants for Saved {
    async fn list(&self, project_id: &str) -> Result<Vec<SavedGrant>, PermissionError> {
        self.list(Some(project_id)).await
    }

    async fn add(
        &self,
        project_id: &str,
        action: &str,
        resources: &[String],
    ) -> Result<(), PermissionError> {
        let project_id = project_id.to_owned();
        let action = action.to_owned();
        let resources = resources.to_vec();
        self.0
            .transaction(TxOptions::default(), move |sql| {
                permission::add(sql, &project_id, &action, &resources)
            })
            .await
            .map_err(port)
    }
}

struct Policy {
    agents: Agents,
    sessions: SessionStore,
}

#[async_trait]
impl PolicySource for Policy {
    async fn configured(
        &self,
        session_id: &str,
        agent: Option<&str>,
    ) -> Result<Option<Ruleset>, PermissionError> {
        let id = SessionID::new(session_id).map_err(port)?;
        let session = self.sessions.get(&id).await.map_err(port)?.ok_or_else(|| {
            PermissionError::SessionNotFound {
                session_id: session_id.to_owned(),
            }
        })?;
        Ok(self
            .agents
            .resolve(agent.or(session.agent.as_ref().map(|id| id.as_str())))
            .map(|agent| {
                agent
                    .permissions
                    .into_iter()
                    .map(|rule| {
                        Rule::new(
                            rule.action,
                            rule.resource,
                            match rule.effect {
                                opencode_schema::permission::Effect::Allow => Effect::Allow,
                                opencode_schema::permission::Effect::Ask => Effect::Ask,
                                opencode_schema::permission::Effect::Deny => Effect::Deny,
                            },
                        )
                    })
                    .collect()
            }))
    }
}

struct Events {
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
}

#[async_trait]
impl PermissionEvents for Events {
    async fn publish(&self, event: PermissionEvent) -> Result<(), PermissionError> {
        let (name, data) = match event {
            PermissionEvent::Asked(request) => (
                "permission.v2.asked",
                serde_json::to_value(request).map_err(port)?,
            ),
            PermissionEvent::Replied {
                session_id,
                request_id,
                reply,
            } => (
                "permission.v2.replied",
                json!({"sessionID": session_id, "requestID": request_id, "reply": reply}),
            ),
        };
        self.events
            .store()
            .publish(
                &Definition::ephemeral(name),
                data,
                PublishOptions {
                    location: Some(self.location.clone()),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(port)?;
        Ok(())
    }
}

fn port(error: impl std::fmt::Display) -> PermissionError {
    PermissionError::Port(error.to_string())
}
