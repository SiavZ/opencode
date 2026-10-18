//! Purpose: workspace sync supervisor (port of packages/opencode/src/control-plane/workspace.ts
//! `connectSSE`, `parseSSE`, `syncHistory`, `syncWorkspaceLoop`, `startSync`, `stopSync`,
//! `waitForSync`, `synced`, `route`).
//! Scope: transport trait with a typed `Unsupported` default and owned sync loops.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/workspace/adapter.rs
//! |-- locations/src/workspace/mod.rs
//! `-- locations/src/workspace/sse.rs
//! External:
//! |-- futures::stream::BoxStream
//! |-- opencode_event_api::{EventStore, ReplayOptions, SerializedEvent}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- locations/tests/workspace_sync.rs
//! Invariants: one live loop per workspace (starting again replaces it); a loop reconnects with
//! backoff `min(120s, 1s * 2^attempt)` reset on every successful connect; history is replayed with
//! `publish: true` and `ownerID = workspace id`, contiguous same-aggregate runs through
//! `replay_all`; a failed live `sync` replay is logged and not forwarded; heartbeats are dropped;
//! every other event is re-emitted on the bus tagged with the workspace. Loops stop on `stop_sync`
//! or service shutdown and are joined.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use opencode_event_api::{ReplayOptions, SerializedEvent};
use opencode_schema::workspace::ConnectionState;
use opencode_storage_api::{DatabaseExt, StorageError, Value};
use serde::Deserialize;
use serde_json::Value as Json;
use tokio_util::sync::CancellationToken;

use super::adapter::{AdapterContext, BusEvent, Info, Target};
pub use super::sse::{SseDecoder, route};
use super::{Shared, SyncTask, WorkspaceService, lock};
use crate::error::{LocationError, Result};

/// One row of `POST /sync/history`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistoryEvent {
    pub id: String,
    pub aggregate_id: String,
    pub seq: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: Json,
}

/// Remote workspace HTTP transport. Owned by Phase 06; the default fails with `Unsupported`.
#[async_trait]
pub trait SyncTransport: Send + Sync {
    /// `GET <url>/global/event` with `accept: text/event-stream`. Non-2xx fails with `SyncHttp`
    /// (`Workspace sync HTTP failure: <status>`).
    async fn connect(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<BoxStream<'static, Result<Vec<u8>>>> {
        let _ = (url, headers);
        Err(LocationError::unsupported(
            "workspace sync transport",
            "remote workspace HTTP/SSE is owned by the server/client phases",
        ))
    }

    /// `POST <url>/sync/history` with the `{ aggregateID: seq }` body. Non-2xx fails with `SyncHttp`
    /// (`Workspace history HTTP failure: <status> <body>`).
    async fn history(
        &self,
        url: &str,
        headers: &[(String, String)],
        state: &BTreeMap<String, i64>,
    ) -> Result<Vec<HistoryEvent>> {
        let _ = (url, headers, state);
        Err(LocationError::unsupported(
            "workspace sync transport",
            "remote workspace HTTP/SSE is owned by the server/client phases",
        ))
    }
}

/// Transport with only the defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedTransport;
impl SyncTransport for UnsupportedTransport {}

/// `startSync`.
pub(crate) async fn start_sync(shared: &Arc<Shared>, space: &Info, context: &AdapterContext) {
    if !shared.options.experimental_workspaces {
        return;
    }
    let target = match shared.adapters.get(&space.project_id, &space.kind) {
        Ok(adapter) => adapter.target(space, context).await,
        Err(error) => Err(error),
    };
    let (url, headers) = match target {
        Err(error) => {
            shared.set_status(&space.id, ConnectionState::Error);
            tracing::warn!(workspaceID = space.id, %error, "workspace target failed");
            return;
        }
        Ok(Target::Local { directory }) => {
            let exists = tokio::fs::try_exists(&directory).await.unwrap_or(false);
            let status = if exists {
                ConnectionState::Connected
            } else {
                ConnectionState::Error
            };
            shared.set_status(&space.id, status);
            return;
        }
        Ok(Target::Remote { url, headers }) => (url, headers),
    };
    let live = lock(&shared.syncs)
        .get(&space.id)
        .is_some_and(|task| !task.handle.is_finished());
    if live && shared.status_of(&space.id) != Some(ConnectionState::Error) {
        return;
    }
    shared.set_status(&space.id, ConnectionState::Disconnected);
    let cancel = shared.shutdown.child_token();
    let loop_shared = shared.clone();
    let loop_space = space.clone();
    let token = cancel.clone();
    let handle = shared.tasks.spawn(async move {
        let run = sync_loop(&loop_shared, &loop_space, &url, &headers, &token);
        let outcome = tokio::select! {
            _ = token.cancelled() => Ok(()),
            outcome = run => outcome,
        };
        if let Err(error) = outcome {
            loop_shared.set_status(&loop_space.id, ConnectionState::Error);
            tracing::warn!(workspaceID = loop_space.id, %error, "workspace listener failed");
        }
    });
    // `FiberMap.run` with an existing key interrupts the previous fiber.
    let previous = lock(&shared.syncs).insert(space.id.clone(), SyncTask { cancel, handle });
    if let Some(previous) = previous {
        previous.cancel.cancel();
        let _ = previous.handle.await;
    }
}

/// `stopSync`.
pub(crate) async fn stop_sync(shared: &Shared, id: &str) {
    let task = lock(&shared.syncs).remove(id);
    if let Some(task) = task {
        task.cancel.cancel();
        let _ = task.handle.await;
    }
    lock(&shared.connections).retain(|(k, _)| k != id);
}

async fn sync_loop(
    shared: &Arc<Shared>,
    space: &Info,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<()> {
    let mut attempt: u32 = 0;
    loop {
        shared.set_status(&space.id, ConnectionState::Connecting);
        let connected = match shared
            .transport
            .connect(&route(url, "/global/event")?, headers)
            .await
        {
            Ok(stream) => sync_history(shared, space, url, headers)
                .await
                .map(|()| stream),
            Err(error) => Err(error),
        };
        match connected {
            Err(error) => {
                shared.set_status(&space.id, ConnectionState::Error);
                tracing::warn!(workspace = space.name, %error, "failed to connect to global sync");
            }
            Ok(mut stream) => {
                attempt = 0;
                shared.set_status(&space.id, ConnectionState::Connected);
                let mut decoder = SseDecoder::new();
                while let Some(chunk) = stream.next().await {
                    for event in decoder.push(&chunk?) {
                        forward(shared, space, event).await;
                    }
                }
                for event in decoder.finish() {
                    forward(shared, space, event).await;
                }
                shared.set_status(&space.id, ConnectionState::Disconnected);
            }
        }
        let delay =
            Duration::from_millis(120_000.min(1_000u64.saturating_mul(1u64 << attempt.min(20))));
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

async fn forward(shared: &Shared, space: &Info, event: Json) {
    let Some(payload) = event.get("payload").filter(|_| event.is_object()) else {
        return;
    };
    let kind = payload.get("type").and_then(Json::as_str);
    if kind == Some("server.heartbeat") {
        return;
    }
    if let (Some("sync"), Some(raw)) = (kind, payload.get("syncEvent")) {
        let replayed = match serde_json::from_value::<SerializedEvent>(raw.clone()) {
            Ok(serialized) => shared
                .events
                .replay(serialized, replay_options(&space.id))
                .await
                .map_err(LocationError::from),
            Err(error) => Err(LocationError::Validation(error.to_string())),
        };
        if let Err(error) = replayed {
            tracing::warn!(workspaceID = space.id, %error, "failed to replay global event");
            return;
        }
    }
    shared.bus.emit(BusEvent {
        directory: event
            .get("directory")
            .and_then(Json::as_str)
            .map(str::to_owned),
        project: event
            .get("project")
            .and_then(Json::as_str)
            .map(str::to_owned),
        workspace: Some(space.id.clone()),
        payload: payload.clone(),
    });
}

fn replay_options(owner: &str) -> ReplayOptions {
    ReplayOptions {
        publish: true,
        owner_id: Some(owner.to_owned()),
        strict_owner: false,
    }
}

/// `syncHistory`: send the local fence of every session in the workspace, replay what is missing.
pub(crate) async fn sync_history(
    shared: &Shared,
    space: &Info,
    url: &str,
    headers: &[(String, String)],
) -> Result<()> {
    let workspace = space.id.clone();
    let state = shared
        .db
        .read(move |sql| {
            let ids: Vec<String> = sql
                .query(
                    "SELECT \"id\" FROM \"session\" WHERE \"workspace_id\" = ?1",
                    &[workspace.into()],
                )?
                .into_iter()
                .map(|row| row.get::<String>("id"))
                .collect::<Result<_, StorageError>>()?;
            sequences(sql, &ids)
        })
        .await?;
    let history = shared
        .transport
        .history(&route(url, "/sync/history")?, headers, &state)
        .await?;
    let mut runs: Vec<Vec<SerializedEvent>> = Vec::new();
    for event in history {
        let serialized = SerializedEvent {
            id: event.id,
            event_type: event.event_type,
            seq: event.seq,
            aggregate_id: event.aggregate_id,
            data: event.data,
        };
        match runs.last_mut() {
            Some(run) if run[0].aggregate_id == serialized.aggregate_id => run.push(serialized),
            _ => runs.push(vec![serialized]),
        }
    }
    for run in runs {
        shared
            .events
            .replay_all(run, replay_options(&space.id))
            .await?;
    }
    Ok(())
}

/// `event_sequence` seq for each id present.
fn sequences(
    sql: &dyn opencode_storage_api::Sql,
    ids: &[String],
) -> Result<BTreeMap<String, i64>, StorageError> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let placeholders = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let params: Vec<Value> = ids.iter().map(Value::from).collect();
    sql.query(
        &format!("SELECT \"aggregate_id\", \"seq\" FROM \"event_sequence\" WHERE \"aggregate_id\" IN ({placeholders})"),
        &params,
    )?
    .into_iter()
    .map(|row| Ok((row.get::<String>("aggregate_id")?, row.get::<i64>("seq")?)))
    .collect()
}

impl WorkspaceService {
    /// `synced(state)`: every aggregate reached its fence (absent counts as -1).
    pub async fn synced(&self, state: &BTreeMap<String, i64>) -> Result<bool> {
        if state.is_empty() {
            return Ok(true);
        }
        let ids: Vec<String> = state.keys().cloned().collect();
        let done = self.shared.db.read(move |sql| sequences(sql, &ids)).await?;
        Ok(state
            .iter()
            .all(|(id, fence)| done.get(id).copied().unwrap_or(-1) >= *fence))
    }

    /// `Workspace.waitForSync`: wake on any bus event for the workspace or of type `sync`.
    /// The timeout restarts after every wake, like the recursive `waitEvent` of the baseline.
    pub async fn wait_for_sync(
        &self,
        workspace_id: &str,
        state: &BTreeMap<String, i64>,
        cancel: Option<CancellationToken>,
        timeout: Option<Duration>,
    ) -> Result<()> {
        let timeout = timeout.unwrap_or(self.shared.options.timeout);
        let cancel = cancel.unwrap_or_default();
        let mut events = self.shared.bus.subscribe();
        if self.synced(state).await? {
            return Ok(());
        }
        let fence = || LocationError::SyncTimeout {
            message: format!(
                "Timed out waiting for sync fence: {}",
                serde_json::to_string(state).unwrap_or_default()
            ),
        };
        loop {
            if cancel.is_cancelled() {
                return Err(aborted());
            }
            let woke = async {
                loop {
                    match events.recv().await {
                        Ok(event)
                            if event.workspace.as_deref() == Some(workspace_id)
                                || event.payload_type() == Some("sync") =>
                        {
                            return true;
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    }
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return Err(aborted()),
                outcome = tokio::time::timeout(timeout, woke) => match outcome {
                    Ok(true) => {}
                    _ => return Err(fence()),
                },
            }
            if self.synced(state).await? {
                return Ok(());
            }
        }
    }
}

fn aborted() -> LocationError {
    LocationError::SyncAborted {
        message: "Request aborted".into(),
    }
}
