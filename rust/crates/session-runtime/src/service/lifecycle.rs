//! Purpose: host-only typed Session lifecycle over shared session rows and projected V2 history.
//! Scope: LocalSessionService rename, deletion and safe visible-history fork extension.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/service.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/service.rs
//! Invariants: fork never copies pending admissions or execution ownership; all copied messages
//! are reconstructed through durable projectors under fresh identities at the source placement.
//! Contract tests: runtime/tests/legacy_lifecycle.rs

use std::collections::{HashMap, HashSet};

use opencode_event_api::{Definition, LocationRef, PublishOptions};
use opencode_schema::session_event::DURABLE_DEFINITIONS;
use opencode_session_api::operations::{CreateRequest, SessionService};
use opencode_session_api::{MessageID, SessionError, SessionID, SessionInfo, SessionResult};
use opencode_storage_api::DatabaseExt;
use serde_json::{Value, json};

use super::LocalSessionService;
use crate::legacy::store::{LegacyStore, object};
use crate::projection::event_error;

impl LocalSessionService {
    pub(super) async fn rename_host(&self, session: &SessionID, title: &str) -> SessionResult<()> {
        self.require(session).await?;
        LegacyStore::new(self.events.clone())
            .set_title(session.as_str(), title)
            .await
            .map_err(legacy_error)
    }

    pub(super) async fn delete_host(&self, session: &SessionID) -> SessionResult<()> {
        self.require(session).await?;
        let mut pending = vec![session.as_str().to_owned()];
        let mut visited = HashSet::new();
        let mut sessions = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let key = id.clone();
            let rows = self
                .events
                .database()
                .read(move |tx| {
                    tx.query("SELECT id FROM session WHERE parent_id = ?", &[key.into()])
                })
                .await
                .map_err(crate::projection::storage_error)?;
            for row in rows {
                pending.push(row.get("id").map_err(crate::projection::storage_error)?);
            }
            sessions.push(id);
        }
        let store = LegacyStore::new(self.events.clone());
        for id in sessions.into_iter().rev() {
            let session = SessionID::new(id.clone())
                .map_err(|error| SessionError::Validation(error.to_string()))?;
            self.execution.interrupt(&session).await;
            let info = store.get(&id).await.map_err(legacy_error)?;
            store
                .publish(
                    &Definition::durable("session.deleted", 1, "sessionID"),
                    json!({"sessionID":id,"info":info}),
                )
                .await
                .map_err(legacy_error)?;
            self.events.store().remove(&id).await.map_err(event_error)?;
        }
        Ok(())
    }

    pub(super) async fn fork_host(
        &self,
        session: &SessionID,
        boundary: Option<&MessageID>,
    ) -> SessionResult<SessionInfo> {
        let source = self.require(session).await?;
        if self.execution.active().await.contains(session) {
            return Err(SessionError::Validation(
                "Cannot fork an active session".into(),
            ));
        }
        let id = session.as_str().to_owned();
        let rows = self
            .events
            .database()
            .read(move |tx| {
                tx.query(
                    "SELECT id, seq FROM session_message WHERE session_id = ? ORDER BY seq",
                    &[id.into()],
                )
            })
            .await
            .map_err(crate::projection::storage_error)?;
        let boundary = match boundary {
            Some(boundary) => Some(
                rows.iter()
                    .position(|row| {
                        row.get::<String>("id")
                            .is_ok_and(|id| id == boundary.as_str())
                    })
                    .ok_or_else(|| SessionError::MessageNotFound {
                        session_id: session.clone(),
                        message_id: boundary.clone(),
                    })?,
            ),
            None => None,
        };
        let selected = rows
            .into_iter()
            .take(boundary.unwrap_or(usize::MAX))
            .map(|row| row.get::<String>("id"))
            .collect::<Result<Vec<_>, _>>()
            .map_err(crate::projection::storage_error)?;
        let ids: HashMap<String, String> = selected
            .into_iter()
            .map(|id| (id, MessageID::create().into_inner()))
            .collect();
        let types = DURABLE_DEFINITIONS
            .iter()
            .filter_map(|definition| {
                definition
                    .durable
                    .map(|durable| format!("{}.{}", definition.r#type, durable.version))
            })
            .collect::<Vec<_>>();
        let mut after = None;
        let mut copied = Vec::new();
        let mut shells = HashSet::new();
        loop {
            let page = self
                .events
                .store()
                .read_aggregate(session.as_str(), after, 100, &types)
                .await
                .map_err(event_error)?;
            for event in page.events {
                after = event.durable.as_ref().map(|durable| durable.seq);
                if event.event_type == "session.next.prompt.admitted" {
                    continue;
                }
                let message = event
                    .data
                    .get("messageID")
                    .or_else(|| event.data.get("assistantMessageID"))
                    .and_then(Value::as_str);
                if message.is_some_and(|message| ids.contains_key(message)) {
                    if event.event_type == "session.next.shell.started"
                        && let Some(call) = event.data.get("callID").and_then(Value::as_str)
                    {
                        shells.insert(call.to_owned());
                    }
                    copied.push(event);
                    continue;
                }
                if event.event_type == "session.next.shell.ended"
                    && event
                        .data
                        .get("callID")
                        .and_then(Value::as_str)
                        .is_some_and(|call| shells.contains(call))
                {
                    copied.push(event);
                }
            }
            if !page.has_more {
                break;
            }
        }
        let target = self
            .create(CreateRequest {
                id: None,
                agent: source.agent.clone(),
                model: source.model.clone(),
                location: source.location.clone(),
            })
            .await?;
        let legacy = LegacyStore::new(self.events.clone());
        let original = legacy.get(session.as_str()).await.map_err(legacy_error)?;
        let title = source.title.as_str();
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
        let mut patch = object(json!({"title":forked}));
        if let Some(metadata) = original.get("metadata") {
            patch.insert("metadata".into(), metadata.clone());
        }
        legacy
            .patch(target.id.as_str(), patch)
            .await
            .map_err(legacy_error)?;
        for event in copied {
            let definition = DURABLE_DEFINITIONS
                .iter()
                .find(|definition| definition.r#type == event.event_type)
                .expect("selected durable event");
            let mut data = event.data;
            data["sessionID"] = json!(target.id);
            for key in ["messageID", "assistantMessageID"] {
                if let Some(mapped) = data
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|id| ids.get(id))
                    .cloned()
                {
                    data[key] = json!(mapped);
                }
            }
            self.events
                .store()
                .publish(
                    &Definition::durable(
                        definition.r#type,
                        definition.durable.expect("durable definition").version,
                        "sessionID",
                    ),
                    data,
                    PublishOptions {
                        metadata: event.metadata,
                        location: Some(LocationRef {
                            directory: target.location.directory.as_str().into(),
                            workspace_id: target
                                .location
                                .workspace_id
                                .as_ref()
                                .map(|id| id.as_str().into()),
                        }),
                        ..PublishOptions::default()
                    },
                )
                .await
                .map_err(event_error)?;
        }
        self.require(&target.id).await
    }
}

fn legacy_error(error: crate::legacy::types::LegacyError) -> SessionError {
    SessionError::Storage {
        message: error.to_string(),
        retryable: matches!(
            error,
            crate::legacy::types::LegacyError::Storage {
                retryable: true,
                ..
            }
        ),
    }
}
