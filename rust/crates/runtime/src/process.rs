//! Purpose: process root. Opens the database at `Database.path()` under the global data dir,
//! installs the event store and Session projectors, builds the process-global Session service and
//! `SessionExecution` (Session-ID based, placement discovered at drain start), the project and
//! control-plane workspace services, the Location registry, and the server `Services` bundle.
//! `serve` mirrors `opencode serve` (packages/opencode/src/cli/cmd/serve.ts) over the full host.
//! Scope: process lifetime: construction, serving and ordered shutdown.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- event-store/src/lib.rs
//! |-- locations/src/git.rs
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/worktree.rs
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/legacy.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/paths.rs
//! |-- schema/src/location.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/values.rs
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/services/mod.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- location::build
//! |-- str::to_owned
//! |-- tokio::fs::canonicalize
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/location.rs
//! Invariants: global services are constructed exactly once per process. Shutdown runs in
//! reverse construction order: listener, Session execution, Location registry (joins every
//! location's tasks), workspace sync loops, database close. Never writes outside `paths`
//! (tests pass a temp root, the binary passes the user's XDG paths).
//! Contract tests: runtime/tests/process.rs

use std::sync::Arc;

use opencode_event_store::SqlEventStore;
use opencode_locations::git::SubprocessGit;
use opencode_locations::ports::{SqlSessionPlacement, WorkspaceRouting};
use opencode_locations::workspace::worktree::WorktreeAdapter;
use opencode_locations::workspace::{
    AdapterContext, AdapterRegistry, WorkspaceBus, WorkspaceOptions,
};
use opencode_locations::{
    GitProjectResolver, LocationRegistry, ProjectResolver, RegistryOptions, WorkspaceService,
};
use opencode_server::app::{ServerConfig, router};
use opencode_server::lifecycle::{ListenError, ListenOptions, Listener, listen};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;
use opencode_server::services::Services;
use opencode_session_runtime::execution::LocalSessionExecution;
use opencode_session_runtime::legacy::run_state::{RunState, SessionStatus};
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::projection::LocalSessionEvents;
use opencode_session_runtime::service::LocalSessionService;
use opencode_session_runtime::store::SessionStore;
use opencode_storage_api::Database;
use opencode_storage_sqlite::{SqliteDatabase, database_path};

use crate::adapters::{ServerLocations, ServerPlacement, ServerWorkspaces, SessionProjects};
use crate::legacy::{LegacyAdapter, LegacyEventBus};
use crate::location::{self, LocationGlobals, Locations, RegistryRunners};

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error(transparent)]
    Session(#[from] opencode_session_api::SessionError),
    #[error(transparent)]
    Permission(#[from] opencode_permissions::PermissionError),
    #[error(transparent)]
    Legacy(#[from] opencode_server::services::legacy::LegacyError),
    #[error("{0}")]
    Service(String),
    #[error("failed to create opencode directories: {0}")]
    Directories(#[from] std::io::Error),
    #[error("failed to open database: {0}")]
    Database(#[from] opencode_storage_api::StorageError),
    #[error(transparent)]
    Location(#[from] opencode_locations::LocationError),
    #[error(transparent)]
    Listen(#[from] ListenError),
}

mod options;
pub use options::ProcessOptions;

/// The running process: global services plus the Location registry.
pub struct Process {
    pub options: ProcessOptions,
    pub database: Arc<dyn Database>,
    pub events: Arc<SqlEventStore>,
    pub session: Arc<LocalSessionService>,
    pub execution: Arc<LocalSessionExecution>,
    pub locations: Locations,
    pub workspaces: WorkspaceService,
    pub projects: Arc<dyn ProjectResolver>,
    pub models: Arc<opencode_provider_adapters::catalog::ModelsService>,
    pub providers: Arc<crate::provider_services::ProviderServices>,
    pub application_tools: opencode_tool_api::ToolRegistry,
    pub global: Arc<crate::global_services::GlobalServices>,
    pub(crate) legacy: Arc<LegacyAdapter>,
    callbacks: Arc<crate::service_router::ServiceRouter>,
    tasks: crate::process_tasks::ProcessTasks,
}

impl Process {
    /// Construct every process-global service. Must run inside a tokio runtime.
    pub async fn start(options: ProcessOptions) -> Result<Self, ProcessError> {
        options.paths.ensure().await?;
        let path = database_path(&options.paths.data, &options.channel, &options.db);
        let database: Arc<dyn Database> = Arc::new(SqliteDatabase::open(&path).await?);
        let events = Arc::new(SqlEventStore::new(database.clone()));
        let session_events = Arc::new(LocalSessionEvents::install(
            database.clone(),
            events.clone(),
        ));
        let mut environment = options.environment.clone();
        environment.vars.extend(options.git_env.iter().cloned());
        let git = Arc::new(SubprocessGit {
            env: environment
                .vars
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        });
        let projects: Arc<dyn ProjectResolver> = Arc::new(GitProjectResolver::new(git.clone()));
        let models_source = environment
            .var("OPENCODE_MODELS_URL")
            .unwrap_or("https://models.opencode.ai");
        let models = Arc::new(
            opencode_provider_adapters::catalog::ModelsService::new(
                reqwest::Client::new(),
                models_source.to_owned(),
                options
                    .paths
                    .cache
                    .join(opencode_provider_adapters::catalog::cache_filename(
                        models_source,
                    )),
                format!("opencode/{}", options.version),
            )
            .with_policy(environment.truthy("OPENCODE_DISABLE_MODELS_FETCH"), None)
            .with_read_path(
                environment
                    .var("OPENCODE_MODELS_PATH")
                    .map(std::path::PathBuf::from),
            ),
        );
        let application_tools =
            opencode_tool_api::ToolRegistry::new(Arc::new(opencode_tool_api::PassthroughBounder));
        let callbacks = Arc::new(crate::service_router::ServiceRouter::default());
        let locations: Locations = LocationRegistry::new(
            Arc::new(LocationGlobals {
                projects: projects.clone(),
                environment,
                events: session_events.clone(),
                paths: options.paths.clone(),
                models: models.clone(),
                application_tools: application_tools.clone(),
                callbacks: callbacks.client(),
            }),
            location::build,
            RegistryOptions {
                idle_ttl: options.idle_ttl,
            },
        );
        let execution = Arc::new(LocalSessionExecution::new(
            Arc::new(SessionStore::new(database.clone())),
            Arc::new(RegistryRunners {
                locations: locations.clone(),
            }),
        ));
        opencode_session_runtime::legacy::projectors::register_projectors(events.as_ref());
        let legacy_store = LegacyStore::new(session_events.clone());
        let run_state = RunState::new(SessionStatus::new(legacy_store.clone()), None);
        let legacy = Arc::new(
            LegacyAdapter::new(
                legacy_store.clone(),
                projects.clone(),
                options.version.clone(),
                run_state.clone(),
            )
            .with_execution(execution.clone())
            .with_locations(Arc::new(
                crate::legacy_location::LegacyLocationResolver::new(
                    locations.clone(),
                    legacy_store,
                    run_state,
                    options.paths.home.clone(),
                    options.version.clone(),
                )
                .with_background(
                    options
                        .environment
                        .truthy("OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS")
                        || options.environment.truthy("OPENCODE_EXPERIMENTAL"),
                ),
            )),
        );
        let session = Arc::new(LocalSessionService::new(
            session_events,
            execution.clone(),
            Arc::new(SessionProjects {
                projects: projects.clone(),
                locations: locations.clone(),
                database: database.clone(),
                events: events.clone(),
            }),
            options.version.clone(),
        ));
        let bus = WorkspaceBus::default();
        let adapters = AdapterRegistry::default();
        adapters.builtin(
            "worktree",
            Arc::new(WorktreeAdapter::new(
                git,
                options.paths.data.clone(),
                bus.clone(),
            )),
        );
        let workspace_transport = Arc::new(crate::workspace_transport::WorkspaceTransport::new(
            reqwest::Client::new(),
        ));
        let providers = Arc::new(crate::provider_services::ProviderServices::new(
            database.clone(),
            models.clone(),
            serde_json::Value::Null,
            options.environment.vars.clone().into_iter().collect(),
        ));
        let workspaces = WorkspaceService::new(
            database.clone(),
            events.clone(),
            adapters,
            bus,
            workspace_transport.clone(),
            Arc::new(crate::workspace_host::Host {
                providers: providers.clone(),
                session: session.clone(),
                legacy: legacy.clone(),
                environment: options.environment.vars.clone(),
            }),
            WorkspaceOptions {
                experimental_workspaces: options.experimental_workspaces,
                configured_workspace_id: options.workspace_id.clone(),
                ..WorkspaceOptions::default()
            },
        );
        let global = Arc::new(
            crate::global_services::GlobalServices::new(
                database.clone(),
                projects.clone(),
                locations.clone(),
                options.clone(),
            )
            .with_workspaces(workspaces.clone())
            .with_events(events.clone())
            .with_workspace_transport(workspace_transport)
            .with_sessions(legacy.clone()),
        );
        let tasks = crate::process_tasks::ProcessTasks::start(
            models.clone(),
            events.clone(),
            options.model_refresh && !options.environment.truthy("OPENCODE_DISABLE_MODELS_FETCH"),
        );
        let process = Self {
            options,
            database,
            events,
            session,
            execution,
            locations,
            workspaces,
            projects,
            models,
            providers,
            application_tools,
            global,
            legacy,
            callbacks,
            tasks,
        };
        process.callbacks.initialize(process.services());
        Ok(process)
    }

    /// Models available to the same location-scoped resolver used by Session execution.
    pub async fn available_models(
        &self,
        directory: std::path::PathBuf,
    ) -> Result<Vec<opencode_schema::provider::ModelInfo>, ProcessError> {
        self.location_services(directory)
            .await?
            .providers
            .catalog()
            .await
            .map(|catalog| catalog.available_models())
            .map_err(ProcessError::Service)
    }

    /// Server services over this process.
    pub fn services(&self) -> Services {
        let placement = SqlSessionPlacement::new(self.database.clone());
        let mut services = Services {
            session: Some(self.session.clone()),
            legacy_sessions: Some(self.legacy.clone()),
            legacy_global: Some(self.global.clone()),
            legacy_native: Some(Arc::new(crate::native_services::http::LegacyNative::new(
                self.locations.clone(),
            ))),
            legacy_providers: Some(Arc::new(crate::process_services::LocationProviders {
                locations: self.locations.clone(),
                global: self.providers.clone(),
            })),
            legacy_integrations: Some(Arc::new(crate::integration_services::IntegrationServices {
                locations: self.locations.clone(),
            })),
            version: self.options.version.clone(),
            ui: Some(Arc::new(crate::ui_assets::Assets {
                client: reqwest::Client::new(),
                upstream: self.options.ui_upstream.clone(),
                embedded: (!self
                    .options
                    .environment
                    .truthy("OPENCODE_DISABLE_EMBEDDED_WEB_UI"))
                .then(|| self.options.ui_assets.clone())
                .flatten(),
            })),
            locations: Some(Arc::new(ServerLocations(Arc::new(self.locations.clone())))),
            placement: Some(Arc::new(ServerPlacement(placement.clone()))),
            workspaces: Some(Arc::new(ServerWorkspaces(WorkspaceRouting::new(
                placement,
                self.workspaces.clone(),
                AdapterContext::default(),
            )))),
            ..Services::default()
        };
        services.streams.events = Some(self.events.clone());
        services.streams.global = Some(Arc::new(LegacyEventBus::new(self.events.clone())));
        services.streams.pty = Some(Arc::new(crate::native_services::NativeProvider {
            locations: self.locations.clone(),
        }));
        services
    }

    /// `Server.listen` with the full host router (legacy and current APIs, CORS, compression).
    pub async fn serve(
        &self,
        listen_options: ListenOptions,
        cors: Vec<String>,
    ) -> Result<Listener, ProcessError> {
        let config = ServerConfig::full_host(
            AuthConfig {
                username: self
                    .options
                    .environment
                    .var("OPENCODE_SERVER_USERNAME")
                    .unwrap_or("opencode")
                    .to_owned(),
                password: self
                    .options
                    .environment
                    .var("OPENCODE_SERVER_PASSWORD")
                    .map(str::to_owned),
            },
            CorsConfig { origins: cors },
        );
        Ok(listen(router(self.services(), config), listen_options).await?)
    }

    /// Ordered teardown, reverse of construction.
    pub async fn shutdown(self) {
        self.tasks.shutdown().await;
        self.legacy.shutdown().await;
        self.execution.shutdown().await;
        self.locations.shutdown().await;
        self.workspaces.shutdown().await;
        self.providers.shutdown().await;
        if let Err(error) = self.database.close().await {
            tracing::warn!(%error, "database close failed");
        }
    }
}
