//! Purpose: global legacy session pagination over real session/project rows.
//! Scope: experimental global list including archived filters and timestamp cursors.
//! Invariants: filters and descending timestamp/ID ordering mirror Session.listGlobal.
//! Contract tests: runtime/tests/legacy_operations.rs

use opencode_server::services::legacy::LegacyResponse;
use opencode_storage_api::{DatabaseExt, Value as Sv};
use serde_json::{Map, Value, json};

use super::{LegacyAdapter, LegacyError, RouteContext, internal, map_error, numbers};

impl LegacyAdapter {
    pub(super) async fn global_page(
        &self,
        context: &RouteContext,
        query: &Map<String, Value>,
    ) -> Result<LegacyResponse, LegacyError> {
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        if query
            .get("directory")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            conditions.push("directory = ?");
            params.push(Sv::from(context.directory.clone()));
        }
        if query.get("roots").and_then(Value::as_bool) == Some(true) {
            conditions.push("parent_id IS NULL");
        }
        for (key, sql) in [
            ("start", "time_updated >= ?"),
            ("cursor", "time_updated < ?"),
        ] {
            if let Some(value) = query
                .get(key)
                .and_then(Value::as_f64)
                .filter(|value| *value != 0.0)
            {
                conditions.push(sql);
                params.push(Sv::from(value));
            }
        }
        if let Some(search) = query
            .get("search")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            conditions.push("title LIKE ?");
            params.push(Sv::from(format!("%{search}%")));
        }
        if query.get("archived").and_then(Value::as_bool) != Some(true) {
            conditions.push("time_archived IS NULL");
        }
        let limit = query.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
        params.push(Sv::from(limit.saturating_add(1) as i64));
        let sql = format!(
            "SELECT id,project_id FROM session {} ORDER BY time_updated DESC,id DESC LIMIT ?",
            if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            }
        );
        let rows = self
            .store
            .events()
            .database()
            .read(move |tx| tx.query(&sql, &params))
            .await
            .map_err(internal)?;
        let more = rows.len() > limit;
        let mut sessions = Vec::new();
        for row in rows.into_iter().take(limit) {
            let mut info = self
                .store
                .get(&row.get::<String>("id").map_err(internal)?)
                .await
                .map_err(map_error)?;
            let project = row.get::<String>("project_id").map_err(internal)?;
            let project = self
                .store
                .events()
                .database()
                .read(move |tx| {
                    tx.query_row(
                        "SELECT id,name,worktree FROM project WHERE id = ?",
                        &[project.into()],
                    )
                })
                .await
                .map_err(internal)?;
            let project = project
                .map(|row| -> Result<Value, LegacyError> {
                    let mut project = Map::new();
                    project.insert(
                        "id".into(),
                        json!(row.get::<String>("id").map_err(internal)?),
                    );
                    if let Some(name) = row.get::<Option<String>>("name").map_err(internal)? {
                        project.insert("name".into(), json!(name));
                    }
                    project.insert(
                        "worktree".into(),
                        json!(row.get::<String>("worktree").map_err(internal)?),
                    );
                    Ok(Value::Object(project))
                })
                .transpose()?
                .unwrap_or(Value::Null);
            info.insert("project".into(), project);
            sessions.push(Value::Object(info));
        }
        let cursor = more
            .then(|| {
                sessions
                    .last()
                    .and_then(|info| info["time"]["updated"].as_i64())
                    .map(|value| value.to_string())
            })
            .flatten();
        Ok(LegacyResponse {
            body: numbers(json!(sessions)),
            status: 200,
            headers: cursor
                .map(|cursor| vec![("x-next-cursor".into(), cursor)])
                .unwrap_or_default(),
        })
    }
}
