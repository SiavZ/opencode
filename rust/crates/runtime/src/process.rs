//! Purpose: process root. Opens the database at `Database.path()` under the global data dir,
//! installs the event store and Session projectors, builds the process-global Session service and
//! `SessionExecution` (Session-ID based, placement discovered at drain start), the project and
//! control-plane workspace services, the Location registry, and the server `Services` bundle.
//! `serve` mirrors `opencode serve` (packages/opencode/src/cli/cmd/serve.ts) over the full host.
//! Scope: process lifetime: construction, serving and ordered shutdown.
//! Uses:
//! rust/crates/
//! |-- event-store/src/store.rs
//! |-- locations/src/git.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/project.rs
//! |-- locations/src/registry.rs
//! |-- locations/src/workspace/mod.rs
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/paths.rs
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/services/mod.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! `-- storage-sqlite/src/connection.rs
//! External:
//! `-- opencode_storage_api::Database
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/tests/process.rs
//! Invariants: global services are constructed exactly once per process. Shutdown runs in
//! reverse construction order: listener, Session execution, Location registry (joins every
//! location's tasks), workspace sync loops, database close. Never writes outside `paths`
//! (tests pass a temp root, the binary passes the user's XDG paths).
//! Contract tests: runtime/tests/process.rs

use std::sync::Arc;
use std::time::Duration;

use opencode_event_store::SqlEventStore;
use opencode_locations::git::SubprocessGit;
use opencode_locations::ports::{SqlSessionPlacement, WorkspaceRouting};
use opencode_locations::workspace::worktree::WorktreeAdapter;
use opencode_locations::workspace::{
    AdapterContext, AdapterRegistry, UnsupportedHost, UnsupportedTransport, WorkspaceBus,
    WorkspaceOptions,
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
use opencode_session_runtime::projection::LocalSessionEvents;
use opencode_session_runtime::service::LocalSessionService;
use opencode_session_runtime::store::SessionStore;
use opencode_storage_api::Database;
use opencode_storage_sqlite::{PathEnv, SqliteDatabase, database_path};

use crate::adapters::{ServerLocations, ServerPlacement, ServerWorkspaces, SessionProjects};
use crate::location::{self, LocationGlobals, Locations, RegistryRunners};
use crate::paths::GlobalPaths;

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("failed to create opencode directories: {0}")]
    Directories(#[from] std::io::Error),
    #[error("failed to open database: {0}")]
    Database(#[from] opencode_storage_api::StorageError),
    #[error(transparent)]
    Location(#[from] opencode_locations::LocationError),
    #[error(transparent)]
    Listen(#[from] ListenError),
}

/// Process inputs. `Default` reads the real environment; tests root everything in a temp dir.
#[derive(Clone)]
pub struct ProcessOptions {
    pub paths: GlobalPaths,
    /// `InstallationChannel` (`OPENCODE_CHANNEL` build constant, `local` in dev builds).
    pub channel: String,
    /// `InstallationVersion`.
    pub version: String,
    pub db: PathEnv,
    /// Location idle TTL (`idleTimeToLive: 60 minutes`).
    pub idle_ttl: Duration,
    /// `RuntimeFlags.experimentalWorkspaces`.
    pub experimental_workspaces: bool,
    /// `OPENCODE_WORKSPACE_ID`.
    pub workspace_id: Option<String>,
    /// Extra environment for `git` (tests isolate `GIT_CONFIG_GLOBAL`).
    pub git_env: Vec<(String, String)>,
    /// Configuration and provider credential environment captured at process construction.
    pub environment: opencode_config::env::Environment,
}

impl std::fmt::Debug for ProcessOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessOptions")
            .field("paths", &self.paths)
            .field("channel", &self.channel)
            .field("version", &self.version)
            .field("db", &self.db)
            .field("idle_ttl", &self.idle_ttl)
            .field("experimental_workspaces", &self.experimental_workspaces)
            .field("workspace_id", &self.workspace_id)
            .field("environment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ProcessOptions {
    pub fn from_process(version: impl Into<String>, channel: impl Into<String>) -> Self {
        let flag = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|value| matches!(value.to_lowercase().as_str(), "true" | "1"))
        };
        Self {
            paths: GlobalPaths::from_process(),
            channel: channel.into(),
            version: version.into(),
            db: PathEnv::from_process(),
            idle_ttl: opencode_locations::DEFAULT_IDLE_TTL,
            experimental_workspaces: flag("OPENCODE_EXPERIMENTAL_WORKSPACES")
                .or_else(|| flag("OPENCODE_EXPERIMENTAL"))
                .unwrap_or(false),
            workspace_id: std::env::var("OPENCODE_WORKSPACE_ID")
                .ok()
                .filter(|id| !id.is_empty()),
            git_env: Vec::new(),
            environment: opencode_config::env::Environment::from_process(),
        }
    }

    /// Everything under `root`, nothing read from the process environment.
    pub fn isolated(root: &std::path::Path) -> Self {
        Self {
            paths: GlobalPaths::rooted(root),
            channel: "local".to_owned(),
            version: "local".to_owned(),
            db: PathEnv::default(),
            idle_ttl: opencode_locations::DEFAULT_IDLE_TTL,
            experimental_workspaces: false,
            workspace_id: None,
            git_env: Vec::new(),
            environment: {
                let mut environment =
                    opencode_config::env::Environment::isolated(root.join("home"));
                environment.global_config = GlobalPaths::rooted(root).config;
                environment
            },
        }
    }
}

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
        let git = Arc::new(SubprocessGit {
            env: options.git_env.clone(),
        });
        let projects: Arc<dyn ProjectResolver> = Arc::new(GitProjectResolver::new(git.clone()));
        let locations: Locations = LocationRegistry::new(
            Arc::new(LocationGlobals {
                projects: projects.clone(),
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
        let session = Arc::new(LocalSessionService::new(
            session_events,
            execution.clone(),
            Arc::new(SessionProjects {
                projects: projects.clone(),
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
        let workspaces = WorkspaceService::new(
            database.clone(),
            events.clone(),
            adapters,
            bus,
            Arc::new(UnsupportedTransport),
            Arc::new(UnsupportedHost),
            WorkspaceOptions {
                experimental_workspaces: options.experimental_workspaces,
                configured_workspace_id: options.workspace_id.clone(),
                ..WorkspaceOptions::default()
            },
        );
        Ok(Self {
            options,
            database,
            events,
            session,
            execution,
            locations,
            workspaces,
            projects,
        })
    }

    /// Models available to the same location-scoped resolver used by Session execution.
    pub async fn available_models(
        &self,
        directory: std::path::PathBuf,
    ) -> Result<Vec<opencode_schema::provider::ModelInfo>, ProcessError> {
        let directory = tokio::fs::canonicalize(directory).await?;
        let location = opencode_schema::location::Ref {
            directory: opencode_schema::values::AbsolutePath::new(directory.to_string_lossy()),
            workspace_id: None,
        };
        Ok(self.locations.get(&location).await?.models.clone())
    }

    /// Server services over this process.
    pub fn services(&self) -> Services {
        let placement = SqlSessionPlacement::new(self.database.clone());
        let mut services = Services {
            session: Some(self.session.clone()),
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
        self.execution.shutdown().await;
        self.locations.shutdown().await;
        self.workspaces.shutdown().await;
        if let Err(error) = self.database.close().await {
            tracing::warn!(%error, "database close failed");
        }
    }
}
