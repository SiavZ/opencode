//! Purpose: expose canonical current agent, command and filesystem operations.
//! Scope: current Location registries and native filesystem, distinct from legacy wire values.
//! Uses:
//! rust/crates/
//! |-- config/src/resources.rs
//! |-- runtime/src/global_services.rs
//! `-- runtime/src/global_services/files.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: the HTTP host supplies Location envelopes; raw reads preserve every byte.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required};
use opencode_platform_api::fs::{FileSystem, FileType};
use opencode_platform_native::fs::NativeFileSystem;
use opencode_server::services::legacy::LegacyBytesResponse;
use std::path::Path;

impl GlobalServices {
    pub(super) async fn current(
        &self,
        operation: &str,
        context: &RouteContext,
        query: &Map<String, Value>,
    ) -> Result<Value, LegacyError> {
        match operation {
            "v2.agent.list" => {
                let lease = self
                    .locations
                    .get(&self.location(context)?)
                    .await
                    .map_err(internal)?;
                serde_json::to_value(lease.agents.all()).map_err(internal)
            }
            "v2.command.list" => {
                let lease = self
                    .locations
                    .get(&self.location(context)?)
                    .await
                    .map_err(internal)?;
                let mut commands = vec![
                    json!({"name":"init","template":include_str!("../../../../../packages/core/src/plugin/command/initialize.txt").replacen("${path}",lease.info.project.directory.as_str(),1),"description":"guided AGENTS.md setup"}),
                    json!({"name":"review","template":include_str!("../../../../../packages/core/src/plugin/command/review.txt").replacen("${path}",lease.info.project.directory.as_str(),1),"description":"review changes [commit|branch|pr], defaults to uncommitted","subtask":true}),
                ];
                for (name, configured) in
                    opencode_config::resources::commands(lease.config.entries())
                {
                    let index = commands.iter().position(|command| command["name"] == name);
                    let command = match index {
                        Some(index) => &mut commands[index],
                        None => {
                            commands.push(json!({"name":name,"template":""}));
                            commands.last_mut().expect("just inserted command")
                        }
                    };
                    for key in ["template", "description", "agent", "subtask"] {
                        if let Some(value) = configured.get(key) {
                            command[key] = value.clone();
                        }
                    }
                    if let Some(model) = configured.get("model").and_then(Value::as_str) {
                        let (provider, id) = model.split_once('/').unwrap_or((model, ""));
                        let variant = command
                            .get("model")
                            .and_then(|model| model.get("variant"))
                            .cloned();
                        command["model"] = json!({"id":id,"providerID":provider});
                        if let Some(variant) = variant {
                            command["model"]["variant"] = variant;
                        }
                    }
                    if let Some(variant) = configured.get("variant")
                        && command.get("model").is_some()
                    {
                        command["model"]["variant"] = variant.clone();
                    }
                }
                Ok(Value::Array(commands))
            }
            "v2.fs.list" => {
                let directory = tokio::fs::canonicalize(&context.directory)
                    .await
                    .map_err(internal)?;
                let path = super::files::target(
                    &directory,
                    query.get("path").and_then(Value::as_str).unwrap_or("."),
                )
                .await?;
                let mut entries = NativeFileSystem.read_dir(&path).await.map_err(internal)?.into_iter().filter_map(|(name, kind)| {
                    let directory_entry = match kind { FileType::File => false, FileType::Directory => true, _ => return None };
                    let relative = path.join(name).strip_prefix(&directory).ok()?.to_string_lossy().into_owned();
                    Some(json!({"path":if directory_entry {format!("{relative}{}",std::path::MAIN_SEPARATOR)} else {relative},"type":if directory_entry {"directory"} else {"file"}}))
                }).collect::<Vec<_>>();
                entries.sort_by(|a, b| {
                    (a["type"] != "directory")
                        .cmp(&(b["type"] != "directory"))
                        .then_with(|| a["path"].as_str().cmp(&b["path"].as_str()))
                });
                Ok(Value::Array(entries))
            }
            "v2.fs.find" => {
                let mut query = query.clone();
                query.entry("limit").or_insert(json!(50));
                let paths = self.files("find.files", context, &query).await?;
                Ok(Value::Array(paths.as_array().into_iter().flatten().filter_map(Value::as_str).map(|path|json!({"path":path,"type":if path.ends_with(std::path::MAIN_SEPARATOR) {"directory"} else {"file"}})).collect()))
            }
            _ => Err(LegacyError::Unavailable(format!(
                "Unknown current global operation: {operation}"
            ))),
        }
    }

    pub(super) async fn current_bytes(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
    ) -> Result<LegacyBytesResponse, LegacyError> {
        if operation != "v2.fs.read" {
            return Err(LegacyError::Unavailable(operation.into()));
        }
        let path =
            super::files::target(Path::new(&context.directory), required(params, "wildcard")?)
                .await?;
        let metadata = NativeFileSystem.metadata(&path).await.map_err(internal)?;
        if metadata.file_type != FileType::File {
            return Err(internal("Path is not a file"));
        }
        Ok(LegacyBytesResponse {
            body: NativeFileSystem.read(&path).await.map_err(internal)?,
            status: 200,
            headers: vec![(
                "content-type".into(),
                opencode_tools::files::read_common::mime_type(&path.to_string_lossy()),
            )],
        })
    }
}
