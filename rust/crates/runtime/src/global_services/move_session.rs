//! Purpose: relocate V2 Sessions through real Git changes and durable Session.Moved events.
//! Scope: process-global orchestration of explicit source and destination directories.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/change.rs
//! |-- session-runtime/src/store.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: apply precedes durable relocation; source reset follows it and preserves index.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, Value, internal, json, required_payload};
use opencode_event_api::{EventStore, PublishOptions};
use opencode_platform_api::git::Git;
use opencode_platform_native::vcs::{NativeGit, change};
use opencode_schema::session_event::Moved;
use opencode_schema::values::{AbsolutePath, DateTimeUtc, RelativePath};
use opencode_session_runtime::projection::definition_of;
use std::path::Path;

impl GlobalServices {
    pub(super) async fn move_session(&self, payload: Option<Value>) -> Result<Value, LegacyError> {
        let payload = required_payload(payload)?;
        let id = payload
            .get("sessionID")
            .and_then(Value::as_str)
            .ok_or_else(|| LegacyError::BadRequest("Missing sessionID".into()))?;
        let session = opencode_schema::SessionID::new(id)
            .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
        let directory = payload
            .pointer("/destination/directory")
            .and_then(Value::as_str)
            .filter(|value| Path::new(value).is_absolute())
            .ok_or_else(|| {
                LegacyError::BadRequest("Missing absolute destination directory".into())
            })?;
        let current = opencode_session_runtime::store::SessionStore::new(self.database.clone())
            .get(&session)
            .await
            .map_err(internal)?
            .ok_or_else(|| move_error(format!("Session not found: {id}")))?;
        if current.location.directory.as_str() == directory {
            return Ok(Value::Null);
        }
        let source = self
            .projects
            .from_directory(current.location.directory.as_str())
            .await
            .map_err(internal)?;
        let destination = self
            .projects
            .from_directory(directory)
            .await
            .map_err(internal)?;
        if current.project_id != destination.project.id {
            return Err(move_error(
                "Destination directory belongs to another project",
            ));
        }
        let git = NativeGit::new(self.git());
        let source_path = Path::new(current.location.directory.as_str());
        let patch = if payload
            .get("moveChanges")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && source.sandbox != destination.sandbox
        {
            let repository = git
                .discover(source_path)
                .await
                .ok_or_else(|| move_error("Source is not a Git repository"))?;
            change::capture(git.cli(), &repository, source_path)
                .await
                .map_err(|error| move_error(error.to_string()))?
        } else {
            String::new()
        };
        if !patch.is_empty() {
            git.discover(Path::new(directory)).await.ok_or_else(||move_error("Unable to apply your changes in the destination directory. The files may conflict with existing changes."))?;
            change::apply(git.cli(),Path::new(directory),&patch).await.map_err(|_|move_error("Unable to apply your changes in the destination directory. The files may conflict with existing changes."))?;
        }
        let relative = Path::new(directory)
            .strip_prefix(&destination.sandbox)
            .map_err(internal)?
            .to_string_lossy()
            .replace('\\', "/");
        let moved = Moved {
            session_id: session,
            timestamp: DateTimeUtc::now(),
            location: opencode_schema::location::Ref {
                directory: AbsolutePath::new(directory),
                workspace_id: None,
            },
            subdirectory: Some(RelativePath::new(relative)),
        };
        let events = self
            .events
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("Event store not composed".into()))?;
        events
            .publish(
                &definition_of::<Moved>(),
                serde_json::to_value(moved).map_err(internal)?,
                PublishOptions::default(),
            )
            .await
            .map_err(internal)?;
        if !patch.is_empty() {
            let repository = git
                .discover(source_path)
                .await
                .ok_or_else(|| move_error("Source is not a Git repository"))?;
            change::discard(
                git.cli(),
                &repository,
                source_path,
                change::IndexMode::Preserve,
                true,
            )
            .await
            .map_err(|error| move_error(error.to_string()))?;
        }
        Ok(Value::Null)
    }
}

fn move_error(message: impl Into<String>) -> LegacyError {
    LegacyError::Response {
        status: 400,
        body: json!({"name":"MoveSessionError","data":{"message":message.into()}}),
    }
}
