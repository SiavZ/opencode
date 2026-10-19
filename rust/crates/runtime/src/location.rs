//! Purpose: the Location-scoped service bundle (`locationServices` in
//! packages/core/src/location-services.ts:42-112), built lazily per `Location.Ref` by
//! `opencode_locations::LocationRegistry` with the 60-minute idle TTL.
//! Scope: bundle type, process-wide inputs hoisted out of the per-location graph, the factory,
//! and the `LocationServiceMap.get(session.location)` runner bridge used at drain start.
//! Uses:
//! rust/crates/
//! |-- config/src/lib.rs
//! |-- context/src/builtins.rs
//! |-- context/src/registry.rs
//! |-- llm-api/src/lib.rs
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- platform-native/src/vcs/mod.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location_agents.rs
//! |-- runtime/src/location_models.rs
//! |-- runtime/src/location_routes.rs
//! |-- runtime/src/location_tools.rs
//! |-- runtime/src/paths.rs
//! |-- runtime/src/process.rs
//! |-- schema/src/location.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/turn.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- builtins::LocationFacts
//! |-- builtins::register
//! |-- builtins::system_today
//! |-- serde_json::Value::as_bool
//! |-- serde_json::Value::as_f64
//! |-- tempfile::tempdir
//! |-- tokio::select
//! |-- tokio::test
//! |-- tokio_util::sync::CancellationToken
//! `-- tracing::info
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/process.rs
//! Invariants: global-tagged services (project resolution, database, events) are shared, never
//! rebuilt per location. `Location.Info` matches `Location.layer(ref)`. Native models, tools,
//! built-in context, compaction and snapshots are Location-scoped. Registry leases and active
//! cancellation cover the complete drain, including transport and tool cleanup.
//! Contract tests: runtime/tests/process.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_config::{CurrentConfig, LocationSnapshot};
use opencode_context::{builtins, registry::Registry};
use opencode_locations::ports::{HasLocationInfo, location_info};
use opencode_locations::{
    BuildContext, LocationError, LocationRegistry, LocationServices, ProjectResolver,
};
use opencode_platform_native::vcs::{GitCli, NativeGit, NativeSnapshots, SnapshotConfig};
use opencode_schema::location::{Info, Ref};
use opencode_schema::provider::ModelInfo;
use opencode_session_api::{SessionError, SessionID};
use opencode_session_runtime::execution::LocationRunners;
use opencode_session_runtime::runner::SessionRunner;
use opencode_session_runtime::runner::model::{LocationModels, ModelCatalog};
use opencode_session_runtime::runner::ports::RegistryContext;
use opencode_session_runtime::runner::turn::{LlmSessionRunner, RunnerDeps};

#[path = "location_agents.rs"]
mod agents;
#[path = "location_models.rs"]
mod model_ports;
#[path = "location_routes.rs"]
mod routes;
#[path = "location_tools.rs"]
mod tools;

/// Process-wide services the per-location builder reads (`Node.tags.values.global`).
pub struct LocationGlobals {
    pub projects: Arc<dyn ProjectResolver>,
    pub environment: opencode_config::Environment,
    pub events: Arc<dyn opencode_session_runtime::projection::SessionEvents>,
    pub paths: crate::paths::GlobalPaths,
}

/// One `Location.Ref`'s services.
pub struct LocationBundle {
    pub info: Info,
    pub models: Vec<ModelInfo>,
    /// `SessionRunnerLLM` for this location.
    pub runner: Arc<dyn SessionRunner>,
    _tools: opencode_tool_api::Registration,
    _context: opencode_context::registry::Registration,
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
    let config = CurrentConfig::load(
        &context.global.environment,
        &LocationSnapshot {
            directory: directory.clone().into(),
            project_directory: resolved.directory.clone(),
        },
    )
    .map_err(build_error)?;
    let (catalog, credentials) = model_ports::load(
        &config,
        &context.global.environment,
        &context.global.paths.cache,
        context.global.events.database(),
    )
    .await
    .map_err(build_error)?;
    let models = catalog.available_models().await;
    let agents = agents::build(
        &config,
        &context.global.paths.home,
        &context.global.paths.data,
        &context.global.paths.tmp,
    );
    let tools = tools::build(
        std::path::Path::new(&directory),
        &resolved.directory,
        &context.global.paths.data,
        &config,
        agents.clone(),
    )
    .await
    .map_err(build_error)?;
    let registry = Registry::default();
    let registration = builtins::register(
        &registry,
        &builtins::LocationFacts {
            directory: directory.clone(),
            project_directory: resolved.directory.display().to_string(),
            git: resolved.vcs.is_some(),
        },
        builtins::system_today(),
    )
    .map_err(build_error)?;
    let llm = Arc::new(opencode_llm_api::RouteClient);
    let settings = opencode_session_runtime::compaction::settings(
        &config
            .documents()
            .map(|document| {
                let value = document.info.get("compaction");
                opencode_session_runtime::compaction::Configured {
                    auto: value
                        .and_then(|v| v.get("auto"))
                        .and_then(serde_json::Value::as_bool),
                    buffer: value
                        .and_then(|v| v.get("buffer"))
                        .and_then(serde_json::Value::as_f64),
                    keep_tokens: value
                        .and_then(|v| v.pointer("/keep/tokens"))
                        .and_then(serde_json::Value::as_f64),
                }
            })
            .collect::<Vec<_>>(),
    );
    let snapshots = NativeSnapshots::open(
        NativeGit::new(GitCli::default().with_env(context.global.environment.vars.clone())),
        SnapshotConfig {
            data_dir: context.global.paths.data.clone(),
            project_id: resolved.id.as_str().to_owned(),
            project_directory: resolved.directory.clone(),
            directory: directory.clone().into(),
            vcs_git: resolved.vcs.is_some(),
            enabled: config
                .latest("snapshots")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        },
    )
    .await;
    let runner = LlmSessionRunner::new(RunnerDeps {
        events: context.global.events.clone(),
        llm: llm.clone(),
        agents,
        tools: Arc::new(tools.materializer),
        models: Arc::new(LocationModels {
            catalog,
            integrations: credentials,
            routes: Arc::new(routes::NativeRoutes::new().map_err(build_error)?),
        }),
        context: Arc::new(RegistryContext(registry)),
        compaction: Arc::new(crate::execution_support::SessionCompaction {
            events: context.global.events.clone(),
            llm,
            settings,
        }),
        snapshots: Arc::new(crate::execution_support::RunnerSnapshots(snapshots)),
        location: context.location.clone(),
    });
    Ok(LocationBundle {
        models,
        info: location_info(
            &context.location,
            &resolved.id,
            &resolved.directory.to_string_lossy(),
        ),
        runner: Arc::new(runner),
        _tools: tools.registration,
        _context: registration,
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

    async fn run_cancellable(
        &self,
        session_id: &SessionID,
        force: bool,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), SessionError> {
        let lease = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SessionError::Cancelled),
            lease = self.locations.get(&self.location) => lease.map_err(|error| SessionError::Run {
                message: error.to_string(),
                retryable: false,
            })?,
        };
        lease
            .runner
            .run_cancellable(session_id, force, cancel)
            .await
    }
}

fn build_error(error: impl std::fmt::Display) -> LocationError {
    LocationError::Build {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{LocationRunners, Ref, RegistryRunners, SessionError, SessionID};
    use crate::process::{Process, ProcessOptions};
    use opencode_schema::values::AbsolutePath;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn cancelled_acquisition_does_not_boot_location_services() {
        let root = tempfile::tempdir().unwrap();
        let process = Process::start(ProcessOptions::isolated(root.path()))
            .await
            .unwrap();
        let runners = RegistryRunners {
            locations: process.locations.clone(),
        };
        let runner = runners.get(&Ref {
            directory: AbsolutePath::new(root.path().canonicalize().unwrap().display().to_string()),
            workspace_id: None,
        });
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            runner
                .run_cancellable(&SessionID::new("ses_pre_cancelled").unwrap(), false, cancel)
                .await,
            Err(SessionError::Cancelled)
        ));
        assert_eq!(process.locations.build_count(), 0);
        assert_eq!(process.locations.len(), 0);
        process.shutdown().await;
    }
}
