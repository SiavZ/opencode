//! Purpose: expose native filesystem, watcher and PTY services using the shared Location event store.
//! Scope: constructor-only Location composition and process HTTP service lookup.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- platform-api/src/pty.rs
//! |-- platform-native/src/fs/mod.rs
//! |-- platform-native/src/pty/mod.rs
//! |-- runtime/src/location.rs
//! |-- server/src/services/streams.rs
//! `-- session-runtime/src/projection.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! `-- none, reserved contract (owner: phase-90, runtime composition)
//! Invariants: HTTP terminals and execution tools use the actual native services, never a second PTY registry.
//! Contract tests: platform-native/tests/pty.rs, runtime/tests/permissions.rs

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use opencode_event_api::{Definition, LocationRef, PublishOptions};
use opencode_platform_api::pty::{PtyError, PtyEvent, PtyService};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use opencode_platform_native::fs::{NativeFileSystem, NativeWatcher};
use opencode_platform_native::pty::{NativePty, PtyEvents};
use opencode_server::services::{PortError, streams::PtyProvider};
use opencode_session_runtime::projection::SessionEvents;
use serde_json::json;

#[path = "native_pty.rs"]
mod leased;

#[path = "native_http.rs"]
pub mod http;

pub struct NativeServices {
    pub pty: Arc<NativePty>,
    pub files: Arc<NativeFileSystem>,
    pub watcher: Arc<NativeWatcher>,
}

pub fn build(
    directory: PathBuf,
    shell: String,
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
) -> NativeServices {
    build_with_environment(
        directory,
        shell,
        events,
        location,
        std::env::vars().collect(),
    )
}

pub fn build_with_environment(
    directory: PathBuf,
    shell: String,
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
    environment: std::collections::BTreeMap<String, String>,
) -> NativeServices {
    NativeServices {
        pty: Arc::new(
            NativePty::new(directory, shell, Arc::new(Events { events, location }))
                .with_environment(environment),
        ),
        files: Arc::new(NativeFileSystem),
        watcher: Arc::new(NativeWatcher::new()),
    }
}

struct Events {
    events: Arc<dyn SessionEvents>,
    location: LocationRef,
}

#[async_trait]
impl PtyEvents for Events {
    async fn publish(&self, event: PtyEvent) -> Result<(), PtyError> {
        let (name, data) = match event {
            PtyEvent::Created(info) => ("pty.created", json!({"info": info})),
            PtyEvent::Updated(info) => ("pty.updated", json!({"info": info})),
            PtyEvent::Exited { id, exit_code } => {
                ("pty.exited", json!({"id": id, "exitCode": exit_code}))
            }
            PtyEvent::Deleted { id } => ("pty.deleted", json!({"id": id})),
        };
        self.events
            .store()
            .publish(
                &Definition::ephemeral(name),
                data,
                PublishOptions {
                    location: Some(self.location.clone()),
                    ..PublishOptions::default()
                },
            )
            .await
            .map_err(|error| {
                PlatformError::new(PlatformErrorKind::Io, "publish", name, error.to_string())
            })?;
        Ok(())
    }
}

pub async fn shells() -> Result<serde_json::Value, opencode_server::services::legacy::LegacyError> {
    let text = tokio::fs::read_to_string("/etc/shells").await.ok();
    Ok(serde_json::Value::Array(
        opencode_tools::shell::select::list(text.as_deref())
            .into_iter()
            .map(|item| json!({"path":item.path,"name":item.name,"acceptable":item.acceptable}))
            .collect(),
    ))
}

pub struct NativeProvider {
    pub locations: crate::location::Locations,
}

#[async_trait]
impl PtyProvider for NativeProvider {
    async fn pty(
        &self,
        directory: &str,
        workspace_id: Option<&str>,
    ) -> Result<Arc<dyn PtyService>, PortError> {
        let location = opencode_schema::location::Ref {
            directory: opencode_schema::values::AbsolutePath::new(directory.to_owned()),
            workspace_id: workspace_id
                .map(opencode_schema::WorkspaceID::new)
                .transpose()
                .map_err(|error| PortError {
                    message: error.to_string(),
                })?,
        };
        let lease = self
            .locations
            .get(&location)
            .await
            .map_err(|error| PortError {
                message: error.to_string(),
            })?;
        Ok(Arc::new(leased::LeasedPty(Arc::new(lease))))
    }
}
