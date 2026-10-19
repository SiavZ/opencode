//! Purpose: legacy session update, fork, recursive deletion, part mutation and pagination adapters.
//! Scope: process adapter using persisted V1 rows and existing durable event projectors.
//! Uses:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- session-runtime/src/legacy/pagination.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy.rs
//! Invariants: fork clones actual legacy messages and remaps parent and compaction references.
//! Contract tests: runtime/tests/legacy_operations.rs

use opencode_event_api::Definition;
use opencode_server::services::legacy::LegacyResponse;
use opencode_session_runtime::legacy::pagination::MessageCursor;
use opencode_session_runtime::legacy::store::object;
use opencode_session_runtime::legacy::types::{message_id, now_millis, part_id};
use opencode_storage_api::DatabaseExt;
use serde_json::{Map, Value, json};

use super::{LegacyAdapter, LegacyError, RouteContext, internal, map_error, numbers, parameter};

impl LegacyAdapter {
    pub(super) async fn message_page(
        &self,
        session: &str,
        query: &Map<String, Value>,
    ) -> Result<LegacyResponse, LegacyError> {
        let before = query
            .get("before")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if before.is_some() && !query.contains_key("limit") {
            return Err(LegacyError::BadRequest("before requires limit".into()));
        }
        let before = before
            .map(MessageCursor::decode)
            .transpose()
            .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
        self.store.get(session).await.map_err(map_error)?;
        let limit = query.get("limit").and_then(Value::as_u64).unwrap_or(0);
        if limit == 0 {
            return serde_json::to_value(self.store.messages(session).await.map_err(map_error)?)
                .map(numbers)
                .map(Into::into)
                .map_err(internal);
        }
        let page = self
            .store
            .page(session, limit as usize, before)
            .await
            .map_err(map_error)?;
        Ok(LegacyResponse {
            body: numbers(serde_json::to_value(page.items).map_err(internal)?),
            status: 200,
            headers: page
                .cursor
                .map(|cursor| {
                    vec![
                        ("X-Next-Cursor".into(), cursor),
                        (
                            "Access-Control-Expose-Headers".into(),
                            "Link, X-Next-Cursor".into(),
                        ),
                    ]
                })
                .unwrap_or_default(),
        })
    }

    pub(super) async fn update(
        &self,
        session: &str,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let current = self.store.get(session).await.map_err(map_error)?;
        let payload = payload
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| LegacyError::BadRequest("Invalid session update".into()))?;
        if let Some(title) = payload.get("title") {
            self.store
                .set_title(
                    session,
                    title
                        .as_str()
                        .ok_or_else(|| LegacyError::BadRequest("Invalid title".into()))?,
                )
                .await
                .map_err(map_error)?;
        }
        if let Some(metadata) = payload.get("metadata") {
            self.store
                .patch(
                    session,
                    object(json!({"metadata":metadata,"time":{"updated":now_millis()}})),
                )
                .await
                .map_err(map_error)?;
        }
        if let Some(permission) = payload.get("permission") {
            let mut merged = current
                .get("permission")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            merged.extend(
                permission
                    .as_array()
                    .ok_or_else(|| LegacyError::BadRequest("Invalid permission".into()))?
                    .iter()
                    .cloned(),
            );
            self.store
                .set_permission(session, json!(merged))
                .await
                .map_err(map_error)?;
        }
        if let Some(archived) = payload.get("time").and_then(|time| time.get("archived")) {
            self.store
                .patch(session, object(json!({"time":{"archived":archived}})))
                .await
                .map_err(map_error)?;
        }
        self.store
            .get(session)
            .await
            .map(Value::Object)
            .map(numbers)
            .map_err(map_error)
    }

    pub(super) async fn children(&self, session: &str) -> Result<Value, LegacyError> {
        self.store.get(session).await.map_err(map_error)?;
        let id = session.to_owned();
        let rows = self
            .store
            .events()
            .database()
            .read(move |tx| tx.query("SELECT id FROM session WHERE parent_id = ?", &[id.into()]))
            .await
            .map_err(internal)?;
        let mut children = Vec::with_capacity(rows.len());
        for row in rows {
            children.push(Value::Object(
                self.store
                    .get(&row.get::<String>("id").map_err(internal)?)
                    .await
                    .map_err(map_error)?,
            ));
        }
        Ok(numbers(Value::Array(children)))
    }

    pub(super) async fn remove(&self, session: &str) -> Result<Value, LegacyError> {
        self.store.get(session).await.map_err(map_error)?;
        let mut pending = vec![session.to_owned()];
        let mut visited = std::collections::HashSet::new();
        let mut sessions = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let info = self.store.get(&id).await.map_err(map_error)?;
            let children = self.children(&id).await?;
            pending.extend(
                children
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|child| child["id"].as_str().map(str::to_owned)),
            );
            sessions.push((id, info));
        }
        for (id, info) in sessions.into_iter().rev() {
            self.run_state.cancel(&id).await;
            self.store
                .publish(
                    &Definition::durable("session.deleted", 1, "sessionID"),
                    json!({"sessionID":id,"info":info}),
                )
                .await
                .map_err(map_error)?;
            self.store
                .events()
                .store()
                .remove(&id)
                .await
                .map_err(internal)?;
        }
        Ok(json!(true))
    }

    pub(super) async fn fork(
        &self,
        context: &RouteContext,
        session: &str,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let original = self.store.get(session).await.map_err(map_error)?;
        let title = original
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let forked = title
            .strip_suffix(')')
            .and_then(|title| title.rsplit_once(" (fork #"))
            .and_then(|(base, number)| {
                number
                    .parse::<u64>()
                    .ok()
                    .and_then(|number| number.checked_add(1))
                    .map(|number| format!("{base} (fork #{number})"))
            })
            .unwrap_or_else(|| format!("{title} (fork #1)"));
        let mut input = object(json!({"title":forked}));
        for key in ["workspaceID", "metadata"] {
            if let Some(value) = original.get(key) {
                input.insert(key.into(), value.clone());
            }
        }
        let created = self.create(context, Some(Value::Object(input))).await?;
        let id = created["id"].as_str().expect("created session ID");
        let messages = self.store.messages(session).await.map_err(map_error)?;
        let target = payload
            .as_ref()
            .and_then(|value| value.get("messageID"))
            .and_then(Value::as_str);
        let count = target
            .and_then(|target| messages.iter().position(|message| message.id() == target))
            .unwrap_or(messages.len());
        let mut ids = std::collections::HashMap::new();
        for message in messages.into_iter().take(count) {
            let next = message_id();
            ids.insert(message.id().to_owned(), next.clone());
            let mut info = message.info;
            if info.get("role").and_then(Value::as_str) == Some("assistant")
                && let Some(parent) = info
                    .get("parentID")
                    .and_then(Value::as_str)
                    .and_then(|parent| ids.get(parent))
            {
                info.insert("parentID".into(), json!(parent));
            }
            info.insert("id".into(), json!(next));
            info.insert("sessionID".into(), json!(id));
            self.store.update_message(&info).await.map_err(map_error)?;
            for mut part in message.parts {
                part.insert("id".into(), json!(part_id()));
                part.insert("messageID".into(), json!(next));
                part.insert("sessionID".into(), json!(id));
                if part.get("type").and_then(Value::as_str) == Some("compaction")
                    && let Some(tail) = part
                        .get("tail_start_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                {
                    match ids.get(&tail) {
                        Some(mapped) => {
                            part.insert("tail_start_id".into(), json!(mapped));
                        }
                        None => {
                            part.shift_remove("tail_start_id");
                        }
                    }
                }
                self.store.update_part(&part).await.map_err(map_error)?;
            }
        }
        Ok(created)
    }

    pub(super) async fn update_part(
        &self,
        session: &str,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        self.store.get(session).await.map_err(map_error)?;
        let part = payload
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| LegacyError::BadRequest("Invalid part".into()))?;
        for (key, expected) in [
            ("id", parameter(params, "partID")?),
            ("messageID", parameter(params, "messageID")?),
            ("sessionID", session),
        ] {
            if part.get(key).and_then(Value::as_str) != Some(expected) {
                return Err(LegacyError::BadRequest(
                    "Part IDs do not match route".into(),
                ));
            }
        }
        self.store.update_part(&part).await.map_err(map_error)?;
        Ok(Value::Object(part))
    }
}
