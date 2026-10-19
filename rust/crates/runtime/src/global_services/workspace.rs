//! Purpose: legacy workspace HTTP operations using the process WorkspaceService.
//! Scope: persisted workspace lifecycle, adapter registry and synchronization supervisor.
//! Uses:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: no synthetic host success; typed adapter failures preserve HTTP error metadata.
//! Contract tests: runtime/tests/global_services.rs

use super::{
    GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required,
    required_payload,
};
use opencode_locations::workspace::{AdapterContext, CreateInput, ProjectContext};

impl GlobalServices {
    pub(super) async fn workspace(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let workspaces = self
            .workspaces
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Workspace service not composed".into()))?;
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?
            .project;
        let adapter_context = AdapterContext {
            project: Some(ProjectContext {
                id: project.id.as_str().into(),
                worktree: project.worktree,
                git: project.vcs.is_some(),
            }),
            workspace_id: context.workspace_id.clone(),
        };
        let id = project.id.as_str();
        match operation {
            "experimental.workspace.adapter.list" => {
                serde_json::to_value(workspaces.adapters().entries(id)).map_err(internal)
            }
            "experimental.workspace.list" => {
                serde_json::to_value(workspaces.list(id).await.map_err(internal)?).map_err(internal)
            }
            "experimental.workspace.status" => {
                let list = workspaces.list(id).await.map_err(internal)?;
                Ok(json!(
                    workspaces
                        .status()
                        .into_iter()
                        .filter(|(id, _)| list.iter().any(|info| info.id == *id))
                        .map(|(id, status)| json!({"workspaceID":id,"status":status}))
                        .collect::<Vec<_>>()
                ))
            }
            "experimental.workspace.create" => {
                let payload = required_payload(payload)?;
                let input = CreateInput {
                    id: payload.get("id").and_then(Value::as_str).map(str::to_owned),
                    kind: payload
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or_else(|| LegacyError::BadRequest("Missing type".into()))?
                        .into(),
                    branch: payload
                        .get("branch")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    extra: payload
                        .get("extra")
                        .filter(|value| !value.is_null())
                        .cloned(),
                    project_id: id.into(),
                };
                let value = workspaces.create(input,&adapter_context).await.map_err(|error|LegacyError::Response {status:500,body:json!({"name":"WorkspaceCreateError","data":{"message":error.to_string()}})})?;
                serde_json::to_value(value).map_err(internal)
            }
            "experimental.workspace.syncList" => {
                workspaces
                    .sync_list(id, &adapter_context)
                    .await
                    .map_err(internal)?;
                Ok(Value::Null)
            }
            "experimental.workspace.remove" => serde_json::to_value(
                workspaces
                    .remove(required(params, "id")?, &adapter_context)
                    .await
                    .map_err(internal)?,
            )
            .map_err(internal),
            "sync.start" => {
                workspaces
                    .start_workspace_syncing(id, &adapter_context)
                    .await
                    .map_err(internal)?;
                Ok(json!(true))
            }
            _ => Err(LegacyError::Unavailable(format!(
                "Workspace operation not yet ported: {operation}"
            ))),
        }
    }
}
