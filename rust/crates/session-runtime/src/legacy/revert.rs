//! Purpose: legacy SessionRevert.revert/unrevert using persisted patch parts and native snapshots.
//! Scope: Location-scoped V1 history boundary and filesystem restoration.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/snapshot.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! `-- session-runtime/src/summary.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy/locations.rs
//! Invariants: busy fails before snapshot effects; a part boundary with no preceding text/tool
//! reverts from the last user, and the first patch for a file wins.
//! Contract tests: session-runtime/tests/legacy_revert.rs

use std::collections::BTreeMap;
use std::sync::Arc;

use opencode_platform_api::snapshot::Snapshots;
use serde_json::{Value, json};

use super::run_state::RunState;
use super::store::{LegacyStore, SESSION_DIFF, object};
use super::types::{LegacyError, Object, part_type, str_field};
use crate::summary::Summary;

pub struct LegacyRevert {
    pub store: LegacyStore,
    pub snapshots: Arc<dyn Snapshots>,
    pub run_state: RunState,
    pub root: std::path::PathBuf,
}

impl LegacyRevert {
    pub async fn revert(
        &self,
        session: &str,
        message: &str,
        part: Option<&str>,
    ) -> Result<Object, LegacyError> {
        self.run_state.assert_not_busy(session)?;
        let all = self.store.messages(session).await?;
        let current = self.store.get(session).await?;
        let mut last_user = None;
        let mut boundary = None;
        let mut files = BTreeMap::new();
        for row in &all {
            if row.is_user() {
                last_user = Some(row.id());
            }
            let mut preceding = false;
            for item in &row.parts {
                if boundary.is_some() {
                    if part_type(item) == "patch"
                        && let Some(hash) = str_field(item, "hash")
                    {
                        for file in item
                            .get("files")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            let path = std::path::Path::new(file);
                            let path = if path.is_absolute() {
                                path.strip_prefix(&self.root).map_err(|_| {
                                    LegacyError::Unknown(format!(
                                        "Patch path is outside the project: {file}"
                                    ))
                                })?
                            } else {
                                path
                            };
                            if path.components().any(|component| {
                                matches!(component, std::path::Component::ParentDir)
                            }) {
                                return Err(LegacyError::Unknown(format!(
                                    "Invalid patch path: {file}"
                                )));
                            }
                            files
                                .entry(path.to_string_lossy().into_owned())
                                .or_insert_with(|| hash.to_owned());
                        }
                    }
                    continue;
                }
                if (row.id() == message && part.is_none())
                    || part.is_some_and(|id| str_field(item, "id") == Some(id))
                {
                    let part_id = preceding.then_some(part).flatten();
                    let message_id = if part_id.is_none() {
                        last_user.unwrap_or(row.id())
                    } else {
                        row.id()
                    };
                    boundary = Some(object(json!({"messageID":message_id})));
                    if let Some(part_id) = part_id {
                        boundary
                            .as_mut()
                            .expect("set boundary")
                            .insert("partID".into(), json!(part_id));
                    }
                }
                preceding |= matches!(part_type(item), "text" | "tool");
            }
        }
        let Some(mut boundary) = boundary else {
            return Ok(current);
        };
        let original = current
            .get("revert")
            .and_then(|revert| revert.get("snapshot"))
            .and_then(Value::as_str);
        let snapshot = match original {
            Some(snapshot) => Some(snapshot.to_owned()),
            None => self.snapshots.capture().await,
        };
        if let Some(snapshot) = original {
            self.snapshots
                .checkout(snapshot)
                .await
                .map_err(snapshot_error)?;
        }
        self.snapshots
            .restore(&files)
            .await
            .map_err(snapshot_error)?;
        if let Some(snapshot) = snapshot {
            let now = self
                .snapshots
                .capture()
                .await
                .unwrap_or_else(|| snapshot.clone());
            let diff = self
                .snapshots
                .diff(&snapshot, &now, None, None)
                .await
                .map_err(snapshot_error)?
                .into_iter()
                .filter_map(|diff| diff.patch)
                .collect::<String>();
            boundary.insert("snapshot".into(), json!(snapshot));
            boundary.insert("diff".into(), json!(diff));
        }
        let index = all
            .iter()
            .position(|row| Some(row.id()) == str_field(&boundary, "messageID"))
            .unwrap_or(all.len());
        let diffs = Summary {
            store: self.store.clone(),
            snapshots: self.snapshots.clone(),
            snapshot: None,
        }
        .compute_diff(&all[index..])
        .await?;
        self.store
            .publish(&SESSION_DIFF, json!({"sessionID":session,"diff":diffs}))
            .await?;
        self.store.patch(session,object(json!({"revert":boundary,"summary":{"additions":diffs.iter().filter_map(|diff|diff["additions"].as_u64()).sum::<u64>(),"deletions":diffs.iter().filter_map(|diff|diff["deletions"].as_u64()).sum::<u64>(),"files":diffs.len()},"time":{"updated":super::types::now_millis()}}))).await?;
        self.store.get(session).await
    }

    pub async fn unrevert(&self, session: &str) -> Result<Object, LegacyError> {
        self.run_state.assert_not_busy(session)?;
        let current = self.store.get(session).await?;
        let Some(revert) = current.get("revert") else {
            return Ok(current);
        };
        if let Some(snapshot) = revert.get("snapshot").and_then(Value::as_str) {
            self.snapshots
                .checkout(snapshot)
                .await
                .map_err(snapshot_error)?;
        }
        self.store.clear_revert(session).await?;
        self.store.get(session).await
    }
}

fn snapshot_error(error: opencode_platform_api::snapshot::SnapshotError) -> LegacyError {
    LegacyError::Unknown(error.to_string())
}
