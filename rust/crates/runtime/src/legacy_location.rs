//! Purpose: resolve genuine Location-scoped legacy Session services for production hosts.
//! Scope: legacy adapters share the native provider, registry, permissions and snapshots.
//! Invariants: each operation holds a Location lease; V1 execution never invokes the V2 runner.
//! Contract tests: runtime/tests/legacy_model.rs

mod adapters;
mod agents;
mod child;
mod prompt_tools;
mod share;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use opencode_llm_api::SystemPart;
use opencode_locations::LocationLease;
use opencode_schema::location::Ref;
use opencode_schema::values::AbsolutePath;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::LegacyError;
use opencode_session_runtime::background::BackgroundJobs;
use opencode_session_runtime::children::{
    ChildLocation, Children, ComposedChildHost, LegacySessions,
};
use opencode_session_runtime::legacy::llm_loop::LlmLegacyLoop;
use opencode_session_runtime::legacy::run_state::RunState;
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::legacy::{Deps, Instance, LegacyConfig, LegacySession};

use crate::legacy::{LegacyLocationServices, LegacyLocations};
use crate::location::{LocationBundle, LocationGlobals, Locations};

type Lease = LocationLease<LocationBundle, LocationGlobals>;

pub struct LegacyLocationResolver {
    locations: Locations,
    store: LegacyStore,
    run_state: RunState,
    home: PathBuf,
    version: String,
    jobs: BackgroundJobs,
    sessions: Mutex<Vec<Children>>,
    background: bool,
}

impl LegacyLocationResolver {
    pub fn with_background(mut self, enabled: bool) -> Self {
        self.background = enabled;
        self
    }
    pub fn new(
        locations: Locations,
        store: LegacyStore,
        run_state: RunState,
        home: PathBuf,
        version: String,
    ) -> Self {
        Self {
            locations,
            store,
            run_state,
            home,
            version,
            jobs: BackgroundJobs::new(),
            sessions: Mutex::new(Vec::new()),
            background: false,
        }
    }
}

#[async_trait::async_trait]
impl LegacyLocations for LegacyLocationResolver {
    async fn background(&self, session: &str) -> Result<bool, LegacyError> {
        if !self.background {
            return Ok(false);
        }
        let jobs = self
            .jobs
            .list()
            .into_iter()
            .filter(|job| {
                job.job_type == "task"
                    && job.status == opencode_session_runtime::background::JobStatus::Running
                    && job.metadata_str("parentSessionId") == Some(session)
                    && !job.is_background()
            })
            .collect::<Vec<_>>();
        let mut promoted = false;
        for job in jobs {
            promoted |= self.jobs.promote(&job.id).await.is_some();
        }
        Ok(promoted)
    }
    async fn cancel_children(&self, session: &str) {
        let children = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        for children in children {
            children.cancel_tree(session).await;
        }
    }
    async fn shutdown(&self) {
        self.jobs.shutdown().await;
        let sessions = std::mem::take(
            &mut *self
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for children in sessions {
            children.shutdown().await;
        }
    }
    async fn get(&self, context: &RouteContext) -> Result<LegacyLocationServices, LegacyError> {
        let location = Ref {
            directory: AbsolutePath::new(context.directory.clone()),
            workspace_id: context
                .workspace_id
                .as_deref()
                .map(opencode_schema::WorkspaceID::new)
                .transpose()
                .map_err(|error| LegacyError::BadRequest(error.to_string()))?,
        };
        let lease = Arc::new(
            self.locations
                .get(&location)
                .await
                .map_err(|error| LegacyError::Internal(error.to_string()))?,
        );
        let config = lease
            .legacy_config
            .port
            .handle(
                "config.get",
                context,
                &serde_json::Map::new(),
                &serde_json::Map::new(),
                None,
            )
            .await?;
        let providers = crate::location_legacy_config::provider_services(
            config.clone(),
            &lease.legacy_config.environment,
            lease.legacy_config.models.clone(),
            lease.legacy_config.database.clone(),
        );
        providers
            .set_plugins(lease.integrations.plugins.clone())
            .await;
        let models = Arc::new(opencode_session_runtime::runner::model::LocationModels {
            catalog: Arc::new(crate::location::model_ports::Catalog(providers.clone())),
            integrations: Arc::new(crate::location::model_ports::Credentials(providers.clone())),
            routes: Arc::new(
                crate::location::routes::NativeRoutes::new()
                    .map_err(|error| LegacyError::Internal(error.to_string()))?,
            ),
        });
        let agents = Arc::new(adapters::Agents::new(
            &config,
            &self.locations.global().paths.data,
            &self.locations.global().paths.tmp,
            std::path::Path::new(lease.info.project.directory.as_str()),
            &lease.integrations.resources.references,
            &lease.integrations.resources.skills,
        ));
        let provider = Arc::new(adapters::Provider(providers));
        let session = Arc::new(OnceLock::new());
        let ops = Arc::new(child::Ops {
            session: session.clone(),
            agents: agents.clone(),
            permissions: lease.permissions.service.clone(),
            store: self.store.clone(),
        });
        let children = Children::new(
            Arc::new(ComposedChildHost::new(
                LegacySessions::new(
                    self.store.events().clone(),
                    ChildLocation {
                        project_id: lease.info.project.id.as_str().into(),
                        directory: context.directory.clone(),
                        path: Some(opencode_session_runtime::service::relative(
                            lease.info.project.directory.as_str(),
                            &context.directory,
                        )),
                        workspace_id: context.workspace_id.clone(),
                        version: self.version.clone(),
                    },
                ),
                ops,
            )),
            self.jobs.clone(),
        );
        let mut settings = child::settings(&config);
        settings.background_subagents = self.background;
        let materializer = Arc::new(child::Materializer {
            base: lease.legacy_materializer.clone(),
            children: children.clone(),
            settings,
        });
        let loaded = lease
            .context
            .load()
            .await
            .map_err(|error| LegacyError::Internal(error.to_string()))?;
        let generation = opencode_context::source::initialize(&loaded)
            .await
            .map_err(|error| LegacyError::Internal(error.to_string()))?;
        let inner = LlmLegacyLoop {
            store: self.store.clone(),
            models,
            llm: lease.llm.clone(),
            tools: Arc::new(
                crate::location_integrations::hooks::IntegrationMaterializer {
                    source: lease.legacy_materializer.clone(),
                    inner: materializer,
                    integrations: lease.integrations.clone(),
                },
            ),
            snapshots: lease.snapshots.clone(),
            agents: agents.clone(),
            provider: provider.clone(),
            system: vec![SystemPart::text(generation.baseline)],
            hooks: Arc::new(prompt_tools::Tools {
                lease: lease.clone(),
            }),
            compaction: config
                .get("compaction")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default(),
        };
        let output = Arc::new(LegacySession::new(Deps {
            store: self.store.clone(),
            run_state: self.run_state.with_jobs(Arc::new(children.clone())),
            agents,
            provider,
            run_loop: Arc::new(child::Loop {
                inner,
                lease: lease.clone(),
                children: children.clone(),
            }),
            tools: Arc::new(prompt_tools::Tools {
                lease: lease.clone(),
            }),
            commands: Arc::new(adapters::Commands(
                lease.integrations.clone(),
                lease.info.project.directory.as_str().into(),
            )),
            share: Arc::new(share::Remote::new(self.store.clone(), &config)),
            fs: lease.native.files.clone(),
            process: Arc::new(opencode_platform_native::process::NativeProcessRunner::new()),
            instance: Instance {
                directory: PathBuf::from(location.directory.as_str()),
                worktree: PathBuf::from(lease.info.project.directory.as_str()),
                home: self.home.clone(),
            },
            config: LegacyConfig {
                shell: config
                    .get("shell")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(if cfg!(windows) {
                        "powershell.exe"
                    } else {
                        "/bin/sh"
                    })
                    .into(),
                share: config
                    .get("share")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                auto_share: false,
                snapshot: config.get("snapshot").and_then(serde_json::Value::as_bool),
            },
        }));
        session
            .set(Arc::downgrade(&output))
            .map_err(|_| LegacyError::Internal("legacy session already composed".into()))?;
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(children);
        Ok(LegacyLocationServices {
            session: output,
            snapshots: lease.snapshots.clone(),
            root: PathBuf::from(lease.info.project.directory.as_str()),
            guard: lease,
        })
    }
}
