//! Purpose: compose the embedded SDK over the real Process and no-auth server router.
//! Scope: one isolated or environment-configured native host, with no listening socket.
//! Uses:
//! rust/crates/
//! |-- client/src/transport/mod.rs
//! |-- runtime/src/lib.rs
//! |-- sdk-embedded/src/embedded/client.rs
//! |-- sdk-embedded/src/embedded/tools.rs
//! |-- server/src/app.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- tokio_util::sync::CancellationToken
//! `-- tokio_util::task::TaskTracker
//! Used by:
//! rust/crates/
//! |-- sdk-embedded/src/embedded/client.rs
//! |-- sdk-embedded/src/embedded/tools.rs
//! `-- sdk-embedded/src/lib.rs
//! Invariants: one Process feeds HTTP and application tools; shutdown joins owned cleanup;
//! dropping the host closes clones immediately and starts cleanup on its live tokio runtime.
//! Contract tests: sdk-embedded/tests/embedded.rs

mod client;
mod tools;

use std::sync::{Arc, Mutex};

use opencode_runtime::{Process, ProcessError};
use opencode_server::app::embedded_router;
use opencode_tool_api::Registration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub use client::EmbeddedClient;
pub use opencode_client::transport::RequestDescriptor;
pub use opencode_runtime::ProcessOptions;
pub use tools::ApplicationTools;

pub const EMBEDDED_BASE_URL: &str = "http://opencode.local";
pub const EMBEDDED_AUTH_USERNAME: &str = "opencode";

/// Construction order retained for consumers inspecting the embedding contract.
pub const REQUIRED_COMPONENTS: [Component; 3] = [
    Component::RuntimeServices,
    Component::EmbeddedRouter,
    Component::InProcessClientTransport,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    RuntimeServices,
    EmbeddedRouter,
    InProcessClientTransport,
}

impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeServices => {
                "opencode-runtime: application tools + saved permissions graph"
            }
            Self::EmbeddedRouter => {
                "opencode-server: embedded no-auth router with disposable handler"
            }
            Self::InProcessClientTransport => "opencode-client: in-process transport",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error(transparent)]
    Process(#[from] ProcessError),
}

struct Lifetime {
    cancel: CancellationToken,
    cleanup: TaskTracker,
    registrations: Mutex<Vec<Registration>>,
}

impl Lifetime {
    fn close(&self) {
        let registrations = {
            let mut registrations = self
                .registrations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.cancel.cancel();
            std::mem::take(&mut *registrations)
        };
        drop(registrations);
    }
}

/// In-process client and process-wide application tool registration over one shared runtime.
/// Call `shutdown().await` for joined disposal. After Drop, `client.wait_closed().await`
/// waits for the same cleanup while the tokio runtime remains alive.
pub struct OpenCode {
    pub client: EmbeddedClient,
    pub tools: ApplicationTools,
    lifetime: Arc<Lifetime>,
}

impl std::fmt::Debug for OpenCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenCode")
            .field("closed", &self.lifetime.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl OpenCode {
    pub async fn create() -> Result<Self, EmbedError> {
        Self::create_with_options(ProcessOptions::from_process("local", "local")).await
    }

    /// Explicit placement avoids reading or writing the caller's real product directories.
    pub async fn create_with_options(options: ProcessOptions) -> Result<Self, EmbedError> {
        let process = Process::start(options).await?;
        let lifetime = Arc::new(Lifetime {
            cancel: CancellationToken::new(),
            cleanup: TaskTracker::new(),
            registrations: Mutex::new(Vec::new()),
        });
        let client = EmbeddedClient::new(embedded_router(process.services()), lifetime.clone());
        let tools = ApplicationTools::new(process.application_tools.clone(), lifetime.clone());
        let cancel = lifetime.cancel.clone();
        // The tracker retains ownership even when the host is dropped; both explicit shutdown
        // and a surviving client's wait_closed join this worker and Process's child tasks.
        lifetime.cleanup.spawn(async move {
            cancel.cancelled().await;
            process.shutdown().await;
        });
        lifetime.cleanup.close();
        Ok(Self {
            client,
            tools,
            lifetime,
        })
    }

    pub async fn shutdown(self) {
        self.lifetime.close();
        self.lifetime.cleanup.wait().await;
    }
}

impl Drop for OpenCode {
    fn drop(&mut self) {
        self.lifetime.close();
    }
}
