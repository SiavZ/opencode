//! Purpose: canonical legacy JSON import matching CLI import placement and conflict semantics.
//! Scope: import {info,messages} into existing session/message/part tables, never V2 history.
//! Uses:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy.rs
//! Invariants: existing IDs retain their content; only the Session placement changes on conflict.
//! Contract tests: runtime/tests/legacy_operations.rs

use opencode_storage_api::{DatabaseExt, TxOptions, Value as Sv};
use serde_json::{Map, Value, json};

use super::{LegacyAdapter, LegacyError, RouteContext, internal, map_error, numbers};

impl LegacyAdapter {
    pub(super) async fn import(
        &self,
        context: &RouteContext,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let payload =
            payload.ok_or_else(|| LegacyError::BadRequest("Missing export data".into()))?;
        let mut info = payload
            .get("info")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| LegacyError::BadRequest("Invalid session export".into()))?;
        for field in ["id", "slug", "title", "version"] {
            if info.get(field).and_then(Value::as_str).is_none() {
                return Err(LegacyError::BadRequest(format!("Invalid session {field}")));
            }
        }
        let messages = payload
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| LegacyError::BadRequest("Invalid exported messages".into()))?;
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?;
        info.insert("projectID".into(), json!(project.project.id));
        info.insert("directory".into(), json!(context.directory));
        info.insert(
            "path".into(),
            json!(opencode_session_runtime::service::relative(
                &project.project.worktree,
                &context.directory
            )),
        );
        let id = info["id"].as_str().expect("validated ID").to_owned();
        let session = id.clone();
        self.store.events().database().transaction(TxOptions::default(), move |tx| {
            let optional_text = |key: &str| info.get(key).and_then(Value::as_str).map(Sv::from).unwrap_or(Sv::Null);
            let number = |path: &[&str], default: f64| path.iter().try_fold(&Value::Object(info.clone()), |value, key| value.get(*key)).and_then(Value::as_f64).unwrap_or(default);
            let mut values = vec![
                ("id",Sv::from(session.as_str())),("project_id",optional_text("projectID")),("directory",optional_text("directory")),("path",optional_text("path")),
                ("slug",optional_text("slug")),("title",optional_text("title")),("version",optional_text("version")),("parent_id",optional_text("parentID")),("workspace_id",optional_text("workspaceID")),("agent",optional_text("agent")),
                ("share_url",info.get("share").and_then(|share|share.get("url")).and_then(Value::as_str).map(Sv::from).unwrap_or(Sv::Null)),
                ("cost",Sv::Real(number(&["cost"],0.0))),
                ("time_created",Sv::Integer(number(&["time","created"],0.0) as i64)),("time_updated",Sv::Integer(number(&["time","updated"],0.0) as i64)),
            ];
            for (column,key) in [("model","model"),("metadata","metadata"),("permission","permission"),("revert","revert")] {
                values.push((column,Sv::json_opt(info.get(key))?));
            }
            for (column,path) in [
                ("tokens_input",vec!["tokens","input"]),("tokens_output",vec!["tokens","output"]),("tokens_reasoning",vec!["tokens","reasoning"]),
                ("tokens_cache_read",vec!["tokens","cache","read"]),("tokens_cache_write",vec!["tokens","cache","write"]),
            ] { values.push((column,Sv::Real(number(&path,0.0)))); }
            for (column,path) in [
                ("time_archived",vec!["time","archived"]),("time_compacting",vec!["time","compacting"]),
                ("summary_additions",vec!["summary","additions"]),("summary_deletions",vec!["summary","deletions"]),("summary_files",vec!["summary","files"]),
            ] {
                let value = path.iter().try_fold(&Value::Object(info.clone()), |value,key|value.get(*key)).and_then(Value::as_f64).map(Sv::Real).unwrap_or(Sv::Null);
                values.push((column,value));
            }
            values.push(("summary_diffs",Sv::json_opt(info.get("summary").and_then(|summary|summary.get("diffs")))?));
            tx.execute(&format!("INSERT INTO session ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,directory=excluded.directory,path=excluded.path",values.iter().map(|(key,_)|*key).collect::<Vec<_>>().join(","),vec!["?";values.len()].join(",")), &values.into_iter().map(|(_,value)|value).collect::<Vec<_>>())?;
            for message in messages {
                let mut data = message.get("info").and_then(Value::as_object).cloned().ok_or_else(|| opencode_storage_api::StorageError::Constraint("Invalid message export".into()))?;
                let message_id = take_id(&mut data,"id")?;
                data.shift_remove("sessionID");
                let created = data.get("time").and_then(|time|time.get("created")).and_then(Value::as_i64).unwrap_or_else(opencode_session_runtime::legacy::types::now_millis);
                tx.execute("INSERT INTO message (id,session_id,time_created,time_updated,data) VALUES (?,?,?,?,?) ON CONFLICT DO NOTHING", &[message_id.as_str().into(),session.as_str().into(),Sv::Integer(created),Sv::Integer(created),Sv::json(&data)?])?;
                for part in message.get("parts").and_then(Value::as_array).into_iter().flatten() {
                    let mut data = part.as_object().cloned().ok_or_else(|| opencode_storage_api::StorageError::Constraint("Invalid part export".into()))?;
                    let part_id = take_id(&mut data,"id")?;
                    let message = take_id(&mut data,"messageID")?;
                    data.shift_remove("sessionID");
                    tx.execute("INSERT INTO part (id,message_id,session_id,time_created,time_updated,data) VALUES (?,?,?,0,0,?) ON CONFLICT DO NOTHING", &[part_id.into(),message.into(),session.as_str().into(),Sv::json(&data)?])?;
                }
            }
            Ok::<_,opencode_storage_api::StorageError>(())
        }).await.map_err(internal)?;
        self.store
            .get(&id)
            .await
            .map(Value::Object)
            .map(numbers)
            .map_err(map_error)
    }
}

fn take_id(
    data: &mut Map<String, Value>,
    key: &str,
) -> Result<String, opencode_storage_api::StorageError> {
    data.shift_remove(key)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| opencode_storage_api::StorageError::Constraint(format!("Invalid {key}")))
}
