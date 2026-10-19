//! Purpose: legacy Session workspace warp over actual targets, replay ownership and HTTP peers.
//! Scope: explicit WorkspaceService and Session cancellation dependencies.
//! Uses:
//! rust/crates/
//! |-- locations/src/workspace/sync.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- reqwest::Client
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: source sync precedes claim; changes apply before transfer; remote steal precedes local update.
//! Contract tests: runtime/tests/global_services.rs

use super::{
    GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required_payload,
};
use opencode_event_api::{EventStore, ReplayOptions, SerializedEvent};
use opencode_locations::workspace::sync::route;
use opencode_locations::workspace::{AdapterContext, Info, Target};
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::projection::LocalSessionEvents;
use opencode_storage_api::DatabaseExt;
use std::collections::BTreeMap;
use std::sync::Arc;

impl GlobalServices {
    pub(super) async fn warp(
        &self,
        context: &RouteContext,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let payload = required_payload(payload)?;
        let session = payload
            .get("sessionID")
            .and_then(Value::as_str)
            .ok_or_else(|| LegacyError::BadRequest("Missing sessionID".into()))?;
        let destination = match payload.get("id") {
            Some(Value::Null) => None,
            Some(Value::String(id)) => Some(id.as_str()),
            _ => return Err(LegacyError::BadRequest("Missing workspace id".into())),
        };
        let workspaces = self
            .workspaces
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Workspace service not composed".into()))?;
        let events = self
            .events
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Event store not composed".into()))?;
        let store = LegacyStore::new(Arc::new(LocalSessionEvents::new(
            self.database.clone(),
            events.clone(),
        )));
        let current = store
            .get(session)
            .await
            .map_err(|error| warp_error(error.to_string()))?;
        let previous = match current.get("workspaceID").and_then(Value::as_str) {
            Some(id) => workspaces.get(id).await.map_err(internal)?,
            None => None,
        };
        if let Some(previous) = &previous {
            match self.workspace_target(previous).await? {
                Target::Remote { url, headers } => {
                    if let Err(error) = self.final_workspace_sync(previous, &url, &headers).await {
                        tracing::warn!(workspaceID=previous.id,sessionID=session,%error,"session warp final source sync failed");
                    }
                }
                Target::Local { .. } => {
                    self.sessions
                        .as_ref()
                        .ok_or_else(|| {
                            LegacyError::Unavailable(
                                "Session cancellation service not composed".into(),
                            )
                        })?
                        .cancel_tree(session)
                        .await?
                }
            }
            events
                .claim(session, destination.unwrap_or(&previous.project_id))
                .await
                .map_err(internal)?;
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(internal)?;
        let patch = if payload
            .get("copyChanges")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            match &previous {
                Some(previous) => match self.workspace_target(previous).await? {
                    Target::Local { directory } => self
                        .vcs(
                            "vcs.diff.raw",
                            &RouteContext {
                                directory,
                                workspace_id: None,
                            },
                            &Map::new(),
                            None,
                        )
                        .await?
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                    Target::Remote { url, headers } => {
                        let response = request(
                            &client,
                            reqwest::Method::GET,
                            &url,
                            "/vcs/diff/raw",
                            &headers,
                            None,
                        )?
                        .send()
                        .await;
                        match response {
                            Ok(response) if response.status().is_success() => {
                                response.text().await.unwrap_or_default()
                            }
                            result => {
                                tracing::warn!(?result, "workspace source patch request failed");
                                String::new()
                            }
                        }
                    }
                },
                None => String::new(),
            }
        } else {
            String::new()
        };
        let target = match destination {
            Some(id) => workspaces.get(id).await.map_err(internal)?,
            None => None,
        };
        if !patch.is_empty() {
            match &target {
                Some(space) => match self.workspace_target(space).await? {
                    Target::Local { directory } => {
                        self.vcs(
                            "vcs.apply",
                            &RouteContext {
                                directory,
                                workspace_id: None,
                            },
                            &Map::new(),
                            Some(json!({"patch":patch})),
                        )
                        .await?;
                    }
                    Target::Remote { url, headers } => {
                        let response = request(
                            &client,
                            reqwest::Method::POST,
                            &url,
                            "/vcs/apply",
                            &headers,
                            Some(&json!({"patch":patch})),
                        )?
                        .send()
                        .await;
                        if !response
                            .as_ref()
                            .is_ok_and(|response| response.status().is_success())
                        {
                            tracing::warn!(?response, "workspace destination patch request failed");
                        }
                    }
                },
                None if destination.is_none() => {
                    self.vcs(
                        "vcs.apply",
                        context,
                        &Map::new(),
                        Some(json!({"patch":patch})),
                    )
                    .await?;
                }
                None => (),
            }
        }
        let Some(destination) = destination else {
            store
                .patch(
                    session,
                    Map::from_iter([("workspaceID".into(), Value::Null)]),
                )
                .await
                .map_err(internal)?;
            return Ok(Value::Null);
        };
        let space = target.ok_or_else(||LegacyError::Response {status:404,body:json!({"name":"NotFoundError","data":{"message":format!("Workspace not found: {destination}")}})})?;
        if let Target::Remote { url, headers } = self.workspace_target(&space).await? {
            let id = session.to_owned();
            let rows = self.database.read(move |sql| {
                sql.query("SELECT id, aggregate_id, seq, type, data FROM event WHERE aggregate_id = ?1 ORDER BY seq ASC",&[id.into()])?.iter().map(|row|Ok(SerializedEvent {id:row.get("id")?,aggregate_id:row.get("aggregate_id")?,seq:row.get("seq")?,event_type:row.get("type")?,data:row.get_json("data")?})).collect::<Result<Vec<_>,opencode_storage_api::StorageError>>()
            }).await.map_err(internal)?;
            if rows.is_empty() {
                return Err(warp_error(format!(
                    "No events found for session: {session}"
                )));
            }
            for batch in rows.chunks(10) {
                let body =
                    json!({"directory":space.directory.as_deref().unwrap_or(""),"events":batch});
                checked(
                    request(
                        &client,
                        reqwest::Method::POST,
                        &url,
                        "/sync/replay",
                        &headers,
                        Some(&body),
                    )?,
                    format!("Failed to warp session {session} into workspace {destination}"),
                )
                .await?;
            }
            checked(
                request(
                    &client,
                    reqwest::Method::POST,
                    &url,
                    "/sync/steal",
                    &headers,
                    Some(&json!({"sessionID":session})),
                )?,
                format!("Failed to steal session {session} into workspace {destination}"),
            )
            .await?;
        }
        store
            .patch(
                session,
                Map::from_iter([("workspaceID".into(), json!(destination))]),
            )
            .await
            .map_err(internal)?;
        Ok(Value::Null)
    }

    async fn workspace_target(&self, space: &Info) -> Result<Target, LegacyError> {
        let project = self
            .projects
            .get(&opencode_schema::ProjectID::new(&space.project_id))
            .await
            .map_err(internal)?;
        let context = AdapterContext {
            project: project.map(|project| opencode_locations::workspace::ProjectContext {
                id: project.id.as_str().into(),
                worktree: project.worktree,
                git: project.vcs.is_some(),
            }),
            workspace_id: Some(space.id.clone()),
        };
        self.workspaces
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Workspace service not composed".into()))?
            .adapters()
            .get(&space.project_id, &space.kind)
            .map_err(internal)?
            .target(space, &context)
            .await
            .map_err(internal)
    }

    async fn final_workspace_sync(
        &self,
        space: &Info,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<(), LegacyError> {
        let transport = self
            .workspace_transport
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Workspace transport not composed".into()))?;
        let id = space.id.clone();
        let state = self.database.read(move |sql| {
            sql.query("SELECT aggregate_id, seq FROM event_sequence WHERE aggregate_id IN (SELECT id FROM session WHERE workspace_id = ?1)",&[id.into()])?.iter().map(|row|Ok((row.get::<String>("aggregate_id")?,row.get::<i64>("seq")?))).collect::<Result<BTreeMap<_,_>,opencode_storage_api::StorageError>>()
        }).await.map_err(internal)?;
        let events = self
            .events
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Event store not composed".into()))?;
        for event in transport
            .history(
                &route(url, "/sync/history").map_err(internal)?,
                headers,
                &state,
            )
            .await
            .map_err(internal)?
        {
            events
                .replay(
                    SerializedEvent {
                        id: event.id,
                        aggregate_id: event.aggregate_id,
                        seq: event.seq,
                        event_type: event.event_type,
                        data: event.data,
                    },
                    ReplayOptions {
                        publish: true,
                        owner_id: Some(space.id.clone()),
                        strict_owner: false,
                    },
                )
                .await
                .map_err(internal)?;
        }
        Ok(())
    }
}

fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    endpoint: &str,
    headers: &[(String, String)],
    body: Option<&Value>,
) -> Result<reqwest::RequestBuilder, LegacyError> {
    let mut request = client.request(method, route(url, endpoint).map_err(internal)?);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    Ok(match body {
        Some(body) => request.json(body),
        None => request,
    })
}

async fn checked(request: reqwest::RequestBuilder, message: String) -> Result<(), LegacyError> {
    let response = request
        .send()
        .await
        .map_err(|error| warp_error(error.to_string()))?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status().as_u16();
    let body = response.text().await.map_err(internal)?;
    Err(warp_error(format!("{message}: HTTP {status} {body}")))
}

fn warp_error(message: impl Into<String>) -> LegacyError {
    LegacyError::Response {
        status: 400,
        body: json!({"name":"WorkspaceWarpError","data":{"message":message.into()}}),
    }
}
