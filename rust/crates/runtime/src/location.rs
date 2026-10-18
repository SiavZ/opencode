//! Purpose: the Location-scoped service bundle (`locationServices` in
//! packages/core/src/location-services.ts:42-112), built lazily per `Location.Ref` by
//! `opencode_locations::LocationRegistry` with the 60-minute idle TTL.
//! Scope: bundle type, process-wide inputs hoisted out of the per-location graph, the factory,
//! and the `LocationServiceMap.get(session.location)` runner bridge used at drain start.
//! Uses:
//! rust/crates/
//! |-- locations/src/ports.rs
//! |-- locations/src/project.rs
//! |-- locations/src/registry.rs
//! |-- session-runtime/src/execution.rs
//! `-- session-runtime/src/runner/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- opencode_schema::location::{Info, Ref}
//! `-- opencode_session_api::{SessionError, SessionID}
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/process.rs
//! Invariants: global-tagged services (project resolution, database, events) are shared, never
//! rebuilt per location. `Location.Info` matches `Location.layer(ref)`. Members whose domain
//! node has not landed are absent, and the runner reports a typed non-retryable failure instead
//! of pretending to run (see rust/contracts/phase-91/composition-notes.md for the member list).
//! Contract tests: runtime/tests/process.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_locations::ports::{HasLocationInfo, location_info};
use opencode_locations::{
    BuildContext, LocationError, LocationRegistry, LocationServices, ProjectResolver,
};
use opencode_schema::location::{Info, Ref};
use opencode_session_api::{SessionError, SessionID};
use opencode_session_runtime::execution::LocationRunners;
use opencode_session_runtime::runner::SessionRunner;

/// Process-wide services the per-location builder reads (`Node.tags.values.global`).
pub struct LocationGlobals {
    pub projects: Arc<dyn ProjectResolver>,
}

/// One `Location.Ref`'s services.
pub struct LocationBundle {
    pub info: Info,
    /// `SessionRunnerLLM` for this location.
    pub runner: Arc<dyn SessionRunner>,
}

impl LocationServices for LocationBundle {}

impl HasLocationInfo for LocationBundle {
    fn location_info(&self) -> &Info {
        &self.info
    }
}

/// `buildLocationServiceMap`'s per-entry build.
pub async fn build(
    context: BuildContext<LocationGlobals>,
) -> Result<LocationBundle, LocationError> {
    let directory = context.location.directory.as_str().to_owned();
    tracing::info!(
        directory = %directory,
        workspaceID = ?context.location.workspace_id.as_ref().map(|id| id.as_str().to_owned()),
        "booting location services"
    );
    let resolved = context.global.projects.resolve(&directory).await?;
    Ok(LocationBundle {
        info: location_info(
            &context.location,
            &resolved.id,
            &resolved.directory.to_string_lossy(),
        ),
        runner: Arc::new(UnportedRunner),
    })
}

/// Registry of location bundles.
pub type Locations = LocationRegistry<LocationBundle, LocationGlobals>;

/// `LocationServiceMap.get(session.location)` at drain start: the lease is held for the whole
/// drain so idle eviction cannot tear the location down underneath a running turn.
pub struct RegistryRunners {
    pub locations: Locations,
}

impl LocationRunners for RegistryRunners {
    fn get(&self, location: &Ref) -> Arc<dyn SessionRunner> {
        Arc::new(LeasedRunner {
            locations: self.locations.clone(),
            location: location.clone(),
        })
    }
}

struct LeasedRunner {
    locations: Locations,
    location: Ref,
}

#[async_trait]
impl SessionRunner for LeasedRunner {
    async fn run(&self, session_id: &SessionID, force: bool) -> Result<(), SessionError> {
        let lease =
            self.locations
                .get(&self.location)
                .await
                .map_err(|error| SessionError::Run {
                    message: error.to_string(),
                    retryable: false,
                })?;
        lease.runner.run(session_id, force).await
    }
}

/// Placeholder member until node p04.execution.turn lands `runner/turn.rs`. Fails every drain
/// with a typed, non-retryable error so callers never observe a fake success.
struct UnportedRunner;

#[async_trait]
impl SessionRunner for UnportedRunner {
    async fn run(&self, _: &SessionID, _: bool) -> Result<(), SessionError> {
        Err(SessionError::Run {
            message: "NotYetPorted: provider-turn runner (p04.execution.turn)".to_owned(),
            retryable: false,
        })
    }
}
