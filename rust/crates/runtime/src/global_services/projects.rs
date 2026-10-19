//! Purpose: persistent project HTTP operations using the real ProjectStore and SQL records.
//! Scope: process database and explicit checkout.
//! Uses:
//! rust/crates/
//! |-- locations/src/project_store.rs
//! |-- runtime/src/global_services.rs
//! `-- storage-api/src/records/crud.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: partial updates preserve omitted fields, missing projects fail explicitly.
//! Contract tests: runtime/tests/global_services.rs

use super::{
    GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required,
    required_payload,
};
use opencode_locations::project_store::info_from_row;
use opencode_storage_api::records::{Crud, ProjectRow};
use opencode_storage_api::{DatabaseExt, TxOptions, Value as SqlValue};

impl GlobalServices {
    pub(super) async fn project(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        match operation {
            "project.current" => serde_json::to_value(
                self.projects
                    .from_directory(&context.directory)
                    .await
                    .map_err(internal)?
                    .project,
            )
            .map_err(internal),
            "project.initGit" => {
                let current = self
                    .projects
                    .from_directory(&context.directory)
                    .await
                    .map_err(internal)?
                    .project;
                if current.vcs.is_some() {
                    return serde_json::to_value(current).map_err(internal);
                }
                let result = self
                    .git()
                    .run(
                        std::path::Path::new(&context.directory),
                        &["init", "--quiet"],
                    )
                    .await;
                if !result.ok() {
                    return Err(internal(
                        result.message("Failed to initialize git repository"),
                    ));
                }
                let next = self
                    .projects
                    .from_directory(&context.directory)
                    .await
                    .map_err(internal)?
                    .project;
                self.locations.invalidate(&self.location(context)?).await;
                serde_json::to_value(next).map_err(internal)
            }
            "project.list" => {
                let rows = self
                    .database
                    .read(|sql| Crud::<ProjectRow>::list(sql, None, &[], None))
                    .await
                    .map_err(internal)?;
                rows.iter()
                    .map(|row| {
                        info_from_row(row)
                            .map_err(internal)
                            .and_then(|info| serde_json::to_value(info).map_err(internal))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            "project.update" => {
                let id = required(params, "projectID")?.to_owned();
                let payload = required_payload(payload)?;
                let mut fields = Vec::new();
                for (key, column) in [("name", "name"), ("commands", "commands")] {
                    if let Some(value) = payload.get(key) {
                        fields.push((column, sql_value(value)?));
                    }
                }
                if let Some(icon) = payload.get("icon") {
                    for (key, column) in [
                        ("url", "icon_url"),
                        ("override", "icon_url_override"),
                        ("color", "icon_color"),
                    ] {
                        if let Some(value) = icon.get(key) {
                            fields.push((column, sql_value(value)?));
                        }
                    }
                }
                let row = self
                    .database
                    .transaction(TxOptions::default(), move |sql| {
                        Crud::<ProjectRow>::update(sql, &[id.into()], &fields)
                    })
                    .await
                    .map_err(internal)?;
                let row = row.ok_or_else(|| LegacyError::Response { status: 404, body: json!({"name":"ProjectNotFoundError","data":{"projectID":params.get("projectID"),"message":format!("Project not found: {}",required(params,"projectID").unwrap_or(""))}}) })?;
                let info = serde_json::to_value(info_from_row(&row).map_err(internal)?)
                    .map_err(internal)?;
                self.publish("project.updated", &context.directory, info.clone())
                    .await?;
                Ok(info)
            }
            "project.directories" => {
                let id = required(params, "projectID")?.to_owned();
                self.database.read(move |sql| {
                    sql.query("SELECT directory, strategy FROM project_directory WHERE project_id = ?1 ORDER BY time_created DESC, directory ASC", &[id.into()])?.into_iter().map(|row| {
                        let mut value = json!({"directory":row.get::<String>("directory")?});
                        if let Some(strategy) = row.get::<Option<String>>("strategy")? { value["strategy"] = json!(strategy); }
                        Ok(value)
                    }).collect::<Result<Vec<_>, opencode_storage_api::StorageError>>()
                }).await.map(Value::Array).map_err(internal)
            }
            _ => Err(LegacyError::Unavailable(format!(
                "Unknown project operation: {operation}"
            ))),
        }
    }
}

fn sql_value(value: &Value) -> Result<SqlValue, LegacyError> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        _ => SqlValue::json(value).map_err(internal),
    }
}
