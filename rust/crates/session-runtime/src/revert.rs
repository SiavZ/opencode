//! Purpose: V2 Session revert stage/clear/commit, porting packages/core/src/session/revert.ts. Plans the
//! file set from assistant snapshots after the boundary, restores files through the Phase 05 snapshot
//! port and publishes the durable `session.next.revert.*` events whose projectors update the Session.
//! Scope: Location-scoped operation over the shared database, snapshot service and event store.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- platform-api/src/snapshot.rs
//! |-- schema/src/revert.rs
//! |-- schema/src/session_event.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/info.rs
//! |-- session-api/src/message.rs
//! |-- storage-api/src/lib.rs
//! `-- session-runtime/src/todo.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/tests/revert.rs
//! Invariants: a missing boundary fails with `MessageNotFound` before any snapshot side effect. The
//! earliest assistant snapshot after the boundary wins per file. An existing staged revert keeps its
//! original snapshot and restores its previously staged files to it first. `files: Some(false)` stages
//! without touching files and records no file diffs. clear/commit on a Session without revert are no-ops.
//! Contract tests: session-runtime/tests/revert.rs

use std::collections::BTreeMap;
use std::sync::Arc;

use opencode_platform_api::snapshot::{DiffStatus, FileDiff as SnapshotDiff, Snapshots};
use opencode_schema::revert::{FileDiff, FileStatus, State};
use opencode_schema::session_event::{RevertCleared, RevertCommitted, RevertStaged};
use opencode_schema::values::{DateTimeUtc, NonNegativeInt, RelativePath};
use opencode_session_api::message::{Message, from_row};
use opencode_session_api::{MessageID, SessionError, SessionID, SessionInfo};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, Value};
use serde_json::Map;

use crate::todo::{Events, publish, storage_error};

/// `SessionRevert` operations bound to one Location's services.
#[derive(Clone)]
pub struct Revert {
    db: Arc<dyn Database>,
    events: Events,
    snapshots: Arc<dyn Snapshots>,
}

impl Revert {
    pub fn new(db: Arc<dyn Database>, events: Events, snapshots: Arc<dyn Snapshots>) -> Self {
        Self {
            db,
            events,
            snapshots,
        }
    }

    /// `SessionRevert.stage`: restore files changed after `message_id` and record the revert state.
    pub async fn stage(
        &self,
        session: &SessionInfo,
        message_id: &MessageID,
        files: Option<bool>,
    ) -> Result<State, SessionError> {
        let staged = session.revert.as_ref();
        let original = match staged.and_then(|revert| revert.snapshot.clone()) {
            Some(snapshot) => Some(snapshot),
            None => self.snapshots.capture().await,
        };
        let next = self.plan(&session.id, message_id).await?;
        let mut restore = BTreeMap::new();
        if let Some(original) = &original {
            staged
                .and_then(|revert| revert.files.as_ref())
                .into_iter()
                .flatten()
                .for_each(|file| {
                    restore.insert(file.path.as_str().to_owned(), original.clone());
                });
        }
        if files != Some(false) {
            next.iter().for_each(|(file, tree)| {
                restore.insert(file.clone(), tree.clone());
            });
        }
        if !restore.is_empty() {
            self.snapshots
                .restore(&restore)
                .await
                .map_err(snapshot_error)?;
        }
        let paths: Vec<String> = match files {
            Some(false) => Vec::new(),
            _ => next.keys().cloned().collect(),
        };
        let diffs = match &original {
            Some(original) => {
                let current = self
                    .snapshots
                    .capture()
                    .await
                    .unwrap_or_else(|| original.clone());
                self.snapshots
                    .diff(original, &current, None, Some(&paths))
                    .await
                    .map_err(snapshot_error)?
                    .into_iter()
                    .map(file_diff)
                    .collect()
            }
            None => Vec::new(),
        };
        let revert = State {
            message_id: message_id.clone(),
            part_id: None,
            snapshot: original,
            diff: Some(
                diffs
                    .iter()
                    .map(|file: &FileDiff| file.patch.as_str())
                    .collect::<String>()
                    .trim()
                    .to_owned(),
            ),
            files: Some(diffs),
        };
        publish(
            &self.events,
            &RevertStaged {
                timestamp: DateTimeUtc::now(),
                session_id: session.id.clone(),
                revert: revert.clone(),
            },
        )
        .await?;
        Ok(revert)
    }

    /// `SessionRevert.clear`: restore staged files to the original snapshot and drop the revert.
    pub async fn clear(&self, session: &SessionInfo) -> Result<(), SessionError> {
        let Some(revert) = &session.revert else {
            return Ok(());
        };
        if let Some(original) = &revert.snapshot {
            let files: BTreeMap<_, _> = revert
                .files
                .iter()
                .flatten()
                .map(|file| (file.path.as_str().to_owned(), original.clone()))
                .collect();
            self.snapshots
                .restore(&files)
                .await
                .map_err(snapshot_error)?;
        }
        publish(
            &self.events,
            &RevertCleared {
                timestamp: DateTimeUtc::now(),
                session_id: session.id.clone(),
            },
        )
        .await
    }

    /// `SessionRevert.commit`: publish the commit; the projector deletes history after the boundary.
    pub async fn commit(&self, session: &SessionInfo) -> Result<(), SessionError> {
        let Some(revert) = &session.revert else {
            return Ok(());
        };
        publish(
            &self.events,
            &RevertCommitted {
                timestamp: DateTimeUtc::now(),
                session_id: session.id.clone(),
                message_id: revert.message_id.clone(),
            },
        )
        .await
    }

    /// `plan`: earliest assistant `snapshot.start` per file after the boundary message.
    async fn plan(
        &self,
        session_id: &SessionID,
        message_id: &MessageID,
    ) -> Result<BTreeMap<String, String>, SessionError> {
        let (session, message) = (session_id.clone(), message_id.clone());
        let rows = self
            .db
            .read(move |tx: &dyn Sql| {
                let boundary = tx.query_row(
                    "SELECT seq FROM session_message WHERE session_id = ? AND id = ?",
                    &[Value::from(session.as_str()), Value::from(message.as_str())],
                )?;
                let Some(boundary) = boundary else {
                    return Ok(None);
                };
                let seq: i64 = boundary.get("seq")?;
                tx.query(
                    "SELECT id, type, data FROM session_message WHERE session_id = ? AND type = 'assistant' AND seq > ? ORDER BY seq ASC",
                    &[Value::from(session.as_str()), Value::Integer(seq)],
                )?
                .iter()
                .map(|row| {
                    Ok((
                        row.get::<String>("id")?,
                        row.get::<String>("type")?,
                        row.get_json::<Map<String, serde_json::Value>>("data")?,
                    ))
                })
                .collect::<Result<Vec<_>, StorageError>>()
                .map(Some)
            })
            .await
            .map_err(storage_error)?;
        let Some(rows) = rows else {
            return Err(SessionError::MessageNotFound {
                session_id: session_id.clone(),
                message_id: message_id.clone(),
            });
        };
        let mut files = BTreeMap::new();
        for (id, kind, data) in rows {
            let row_message_id = MessageID::new(id.clone())
                .map_err(|error| SessionError::Validation(error.to_string()))?;
            let decoded = from_row(&id, &kind, data).map_err(|_| SessionError::MessageDecode {
                session_id: session_id.clone(),
                message_id: row_message_id,
            })?;
            let Message::Assistant(assistant) = decoded else {
                continue;
            };
            let Some(snapshot) = assistant.snapshot else {
                continue;
            };
            let Some(start) = snapshot.start else {
                continue;
            };
            snapshot
                .files
                .into_iter()
                .flatten()
                .for_each(|file: RelativePath| {
                    files
                        .entry(file.as_str().to_owned())
                        .or_insert_with(|| start.clone());
                });
        }
        Ok(files)
    }
}

/// Phase 05 returns the legacy `SnapshotFileDiff` shape (optional file/patch/status); V2 Snapshot.diff
/// yields `File.Diff` with required fields. Missing values map to empty path/patch and `modified`.
fn file_diff(diff: SnapshotDiff) -> FileDiff {
    FileDiff {
        path: RelativePath::new(diff.file.unwrap_or_default()),
        status: match diff.status {
            Some(DiffStatus::Added) => FileStatus::Added,
            Some(DiffStatus::Deleted) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        },
        additions: count(diff.additions),
        deletions: count(diff.deletions),
        patch: diff.patch.unwrap_or_default(),
    }
}

fn count(value: u64) -> NonNegativeInt {
    NonNegativeInt::new(value.min(i64::MAX as u64) as i64).unwrap_or(NonNegativeInt::ZERO)
}

fn snapshot_error(error: opencode_platform_api::snapshot::SnapshotError) -> SessionError {
    SessionError::Run {
        message: error.to_string(),
        retryable: false,
    }
}
