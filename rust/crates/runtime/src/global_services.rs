//! Purpose: concrete legacy global HTTP adapter over configuration, projects and native domains.
//! Scope: process-global service with explicit request Location and captured environment.
//! Uses:
//! rust/crates/
//! |-- runtime/src/global_services/config.rs
//! |-- runtime/src/global_services/files.rs
//! |-- runtime/src/global_services/projects.rs
//! |-- runtime/src/global_services/vcs.rs
//! |-- runtime/src/location.rs
//! `-- runtime/src/process.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: all replies originate from real persisted domains or native operations.
//! Contract tests: runtime/tests/global_services.rs

mod agents;
mod commands;
mod config;
mod events;
mod files;
mod move_session;
mod projects;
mod tui;
mod vcs;
mod warp;
mod workspace;

use crate::location::Locations;
use crate::process::ProcessOptions;
use opencode_locations::{ProjectResolver, ProjectStore};
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyError, LegacyResponse, LegacySessions};
use opencode_storage_api::Database;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub use config::RemoteConfiguration;
pub use tui::TuiControl;

/// Concrete process service. Construction performs no I/O and reads no process environment.
pub struct GlobalServices {
    tui: TuiControl,
    database: Arc<dyn Database>,
    projects: ProjectStore,
    locations: Locations,
    options: ProcessOptions,
    sessions: Option<Arc<crate::legacy::LegacyAdapter>>,
    workspace_transport: Option<Arc<dyn opencode_locations::workspace::sync::SyncTransport>>,
    workspaces: Option<opencode_locations::WorkspaceService>,
    events: Option<Arc<opencode_event_store::SqlEventStore>>,
}

impl GlobalServices {
    pub fn new(
        database: Arc<dyn Database>,
        projects: Arc<dyn ProjectResolver>,
        locations: Locations,
        options: ProcessOptions,
    ) -> Self {
        Self {
            tui: TuiControl::default(),
            projects: ProjectStore::new(database.clone(), projects),
            database,
            locations,
            options,
            sessions: None,
            workspace_transport: None,
            workspaces: None,
            events: None,
        }
    }

    pub fn with_workspaces(mut self, workspaces: opencode_locations::WorkspaceService) -> Self {
        self.workspaces = Some(workspaces);
        self
    }

    pub fn with_events(mut self, events: Arc<opencode_event_store::SqlEventStore>) -> Self {
        self.events = Some(events);
        self
    }

    pub fn with_sessions(mut self, sessions: Arc<crate::legacy::LegacyAdapter>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    pub fn with_workspace_transport(
        mut self,
        transport: Arc<dyn opencode_locations::workspace::sync::SyncTransport>,
    ) -> Self {
        self.workspace_transport = Some(transport);
        self
    }

    pub async fn resolved_config(&self, directory: &str) -> Result<Value, LegacyError> {
        self.config(directory).await.map(|state| state.config)
    }

    fn location(
        &self,
        context: &RouteContext,
    ) -> Result<opencode_schema::location::Ref, LegacyError> {
        Ok(opencode_schema::location::Ref {
            directory: opencode_schema::values::AbsolutePath::new(context.directory.clone()),
            workspace_id: context
                .workspace_id
                .as_ref()
                .map(opencode_schema::ids::WorkspaceID::new)
                .transpose()
                .map_err(|error| LegacyError::BadRequest(error.to_string()))?,
        })
    }
}

#[async_trait::async_trait]
impl LegacySessions for GlobalServices {
    async fn handle_response(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<LegacyResponse, LegacyError> {
        let body = self
            .handle(operation, context, params, query, payload)
            .await?;
        Ok(LegacyResponse {
            body,
            status: 200,
            headers: if operation == "vcs.diff.raw" {
                vec![("content-type".into(), "text/plain; charset=utf-8".into())]
            } else {
                Vec::new()
            },
        })
    }

    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        match operation {
            "global.config.get" => self.global_config().await,
            "global.config.update" => self.update_global(required_payload(payload)?).await,
            "config.get" => self.resolved_config(&context.directory).await,
            "config.update" => {
                let payload = required_payload(payload)?;
                opencode_config::legacy::update(
                    std::path::Path::new(&context.directory),
                    &payload,
                    &self.options.environment,
                )
                .map_err(config::error)?;
                self.locations.invalidate(&self.location(context)?).await;
                Ok(payload)
            }
            "instance.dispose" => {
                self.locations.invalidate(&self.location(context)?).await;
                self.locations.wait_teardowns().await;
                self.publish(
                    "server.instance.disposed",
                    &context.directory,
                    json!({"directory":context.directory}),
                )
                .await?;
                Ok(json!(true))
            }
            "global.dispose" => {
                self.locations.invalidate_all().await;
                self.publish("global.disposed", "global", json!({})).await?;
                Ok(json!(true))
            }
            "path.get" => {
                let project = self
                    .projects
                    .from_directory(&context.directory)
                    .await
                    .map_err(internal)?;
                Ok(
                    json!({"home":self.options.paths.home,"state":self.options.paths.state,"config":self.options.paths.config,"worktree":project.sandbox,"directory":context.directory}),
                )
            }
            operation if operation.starts_with("project.") => {
                self.project(operation, context, params, payload).await
            }
            operation if operation.starts_with("file.") || operation.starts_with("find.") => {
                self.files(operation, context, query).await
            }
            operation if operation.starts_with("vcs.") => {
                self.vcs(operation, context, query, payload).await
            }
            "experimental.controlPlane.moveSession" => self.move_session(payload).await,
            operation if operation.starts_with("tui.") => {
                self.tui(operation, context, payload).await
            }
            "experimental.workspace.warp" => self.warp(context, payload).await,
            "command.list" => self.commands(context).await,
            operation
                if operation.starts_with("experimental.workspace.")
                    || operation == "sync.start" =>
            {
                self.workspace(operation, context, params, payload).await
            }
            operation if operation.starts_with("sync.") => {
                self.sync(operation, context, payload).await
            }
            "experimental.capabilities.get" => Ok(
                json!({"backgroundSubagents":self.options.environment.var("OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS").or_else(||self.options.environment.var("OPENCODE_EXPERIMENTAL")).is_some_and(|value|value == "true" || value == "1")}),
            ),
            "app.agents" | "agent.list" => self.agents(context).await,
            "app.log" => {
                let payload = required_payload(payload)?;
                let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
                let service = payload.get("service").and_then(Value::as_str).unwrap_or("");
                let extra = payload.get("extra");
                match payload.get("level").and_then(Value::as_str) {
                    Some("debug") => tracing::debug!(service, ?extra, "{message}"),
                    Some("warn") => tracing::warn!(service, ?extra, "{message}"),
                    Some("error") => tracing::error!(service, ?extra, "{message}"),
                    _ => tracing::info!(service, ?extra, "{message}"),
                }
                Ok(json!(true))
            }
            _ => Err(LegacyError::Unavailable(format!(
                "Global operation not implemented: {operation}"
            ))),
        }
    }
}

fn required_payload(payload: Option<Value>) -> Result<Value, LegacyError> {
    payload
        .filter(Value::is_object)
        .ok_or_else(|| LegacyError::BadRequest("Expected an object payload".into()))
}

fn required<'a>(values: &'a Map<String, Value>, key: &str) -> Result<&'a str, LegacyError> {
    values
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| LegacyError::BadRequest(format!("Missing {key}")))
}

fn internal(error: impl std::fmt::Display) -> LegacyError {
    LegacyError::Internal(error.to_string())
}
