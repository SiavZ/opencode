//! Purpose: create one MCP connection from config, port of `create`, `connectLocal`,
//! `connectRemote` and `connectTransport` (baseline index.ts:218-415): stdio for local servers,
//! Streamable HTTP then legacy SSE for remote ones, connect timeout, status classification
//! (connected / failed / needs_auth / needs_client_registration) and initial tool listing.
//! Scope: stateless; the service stores the result.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/stdio.rs
//! |-- integration-adapters/src/mcp/streamable.rs
//! |-- integration-adapters/src/mcp/transport.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- catalog::defs
//! |-- reqwest::Client
//! |-- tokio::sync::mpsc::Receiver
//! |-- tokio::time::timeout
//! |-- tracing::debug
//! |-- tracing::warn
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/service.rs
//! `-- integration-adapters/src/mcp/service_auth.rs
//! Invariants: a failed connect closes its transport; an auth failure stops the transport
//! fallback; a needs_auth result returns the transport for `finish_auth`; tool listing failure
//! closes the client and yields failed "Failed to get tools".
//! Contract tests: integration-adapters/tests/mcp_stdio.rs, integration-adapters/tests/mcp_http.rs

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use opencode_integration_api::mcp::{
    DEFAULT_TIMEOUT_MS, LocalConfig, OAuthSetting, RemoteConfig, ServerConfig, Status, ToolDef,
};

use super::auth_store::McpAuthStore;
use super::catalog;
use super::client::McpClient;
use super::jsonrpc::RpcError;
use super::legacy_sse::LegacySseTransport;
use super::oauth_provider::{OAuthProvider, ProviderConfig};
use super::remote::RemoteOptions;
use super::stdio::{StdioParams, StdioTransport};
use super::streamable::StreamableTransport;
use super::transport::{Transport, TransportError};

pub struct Connected {
    pub client: Arc<McpClient>,
    pub defs: Vec<ToolDef>,
    pub instructions: Option<String>,
}

/// A remote transport kept alive so `finish_auth` can exchange the code on it.
#[derive(Clone)]
pub enum AuthTransport {
    Streamable(Arc<StreamableTransport>),
    Sse(Arc<LegacySseTransport>),
}

impl AuthTransport {
    pub async fn finish_auth(&self, code: &str) -> Result<(), TransportError> {
        match self {
            Self::Streamable(t) => t.finish_auth(code).await,
            Self::Sse(t) => t.finish_auth(code).await,
        }
    }
}

pub struct PendingAuth {
    pub transport: AuthTransport,
    pub provider: Option<Arc<OAuthProvider>>,
}

pub struct CreateResult {
    pub status: Status,
    pub connected: Option<Connected>,
    pub pending: Option<PendingAuth>,
    /// Toast the baseline shows for auth-required outcomes.
    pub toast: Option<String>,
}

impl CreateResult {
    fn status(status: Status) -> Self {
        Self {
            status,
            connected: None,
            pending: None,
            toast: None,
        }
    }
}

pub struct ConnectEnv<'a> {
    pub startup_timeouts: &'a BTreeMap<String, u64>,
    pub environment: &'a BTreeMap<String, String>,
    pub directory: &'a Path,
    pub store: &'a McpAuthStore,
    pub http: &'a reqwest::Client,
}

pub fn provider_config(remote: &RemoteConfig) -> ProviderConfig {
    match &remote.oauth {
        Some(OAuthSetting::Config(c)) => ProviderConfig {
            client_id: c.client_id.clone(),
            client_secret: c.client_secret.clone(),
            scope: c.scope.clone(),
            callback_port: c.callback_port,
            redirect_uri: c.redirect_uri.clone(),
        },
        _ => ProviderConfig::default(),
    }
}

/// `create(key, mcp)`.
pub async fn create(env: &ConnectEnv<'_>, key: &str, config: &ServerConfig) -> CreateResult {
    if !config.enabled() {
        return CreateResult::status(Status::Disabled);
    }
    let mut result = match config {
        ServerConfig::Remote(r) => connect_remote(env, key, r).await,
        ServerConfig::Local(l) => connect_local(env, key, l).await,
    };
    let Some(connected) = result.connected.take() else {
        if !matches!(result.status, Status::Connected | Status::Disabled) {
            tracing::warn!(key, status = ?result.status, "server unavailable");
        }
        return result;
    };
    let client = connected.client;
    let listed = if client.has_capability("tools") {
        catalog::defs(&client, config.timeout_ms()).await
    } else {
        Some(Vec::new())
    };
    let Some(defs) = listed else {
        client.close().await;
        return CreateResult::status(Status::Failed {
            error: "Failed to get tools".into(),
        });
    };
    result.connected = Some(Connected {
        instructions: client.instructions(),
        client,
        defs,
    });
    result
}

/// `connectTransport`: handshake under the connect timeout. The caller's transport is closed on
/// failure (the handshake already closes on protocol errors; timeouts close here).
pub async fn connect_transport(
    transport: Arc<dyn Transport>,
    inbound: tokio::sync::mpsc::Receiver<super::transport::Inbound>,
    directory: &Path,
    timeout_ms: u64,
) -> Result<McpClient, RpcError> {
    let fut = McpClient::connect(transport.clone(), inbound, directory);
    match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(r) => r,
        Err(_) => {
            transport.close().await;
            Err(RpcError::Other(format!(
                "Operation timed out after {timeout_ms}ms"
            )))
        }
    }
}

async fn connect_local(env: &ConnectEnv<'_>, key: &str, local: &LocalConfig) -> CreateResult {
    let Some((cmd, args)) = local.command.split_first() else {
        return CreateResult::status(Status::Failed {
            error: "MCP command is empty".into(),
        });
    };
    let cwd = match &local.cwd {
        Some(c) => env.directory.join(c),
        None => env.directory.to_path_buf(),
    };
    let mut env_map = env.environment.clone();
    if cmd == "opencode" {
        env_map.insert("BUN_BE_BUN".into(), "1".into());
    }
    if let Some(e) = &local.environment {
        env_map.extend(e.clone());
    }
    let spawned = StdioTransport::spawn(StdioParams {
        command: cmd.clone(),
        args: args.to_vec(),
        cwd,
        env: env_map,
    });
    let (transport, inbound) = match spawned {
        Ok(t) => t,
        Err(e) => {
            return CreateResult::status(Status::Failed {
                error: e.to_string(),
            });
        }
    };
    let timeout = env
        .startup_timeouts
        .get(key)
        .copied()
        .or(local.timeout)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    match connect_transport(Arc::new(transport), inbound, env.directory, timeout).await {
        Ok(client) => connected(client),
        Err(e) => CreateResult::status(Status::Failed {
            error: e.to_string(),
        }),
    }
}

fn connected(client: McpClient) -> CreateResult {
    CreateResult {
        status: Status::Connected,
        connected: Some(Connected {
            client: Arc::new(client),
            defs: Vec::new(),
            instructions: None,
        }),
        pending: None,
        toast: None,
    }
}

async fn connect_remote(env: &ConnectEnv<'_>, key: &str, remote: &RemoteConfig) -> CreateResult {
    let Ok(url) = url::Url::parse(&remote.url) else {
        return CreateResult::status(Status::Failed {
            error: format!("Invalid MCP URL for \"{key}\""),
        });
    };
    let oauth_disabled = matches!(remote.oauth, Some(OAuthSetting::Disabled(_)));
    let provider = (!oauth_disabled).then(|| {
        Arc::new(OAuthProvider::persistent(
            key,
            &remote.url,
            provider_config(remote),
            env.store.clone(),
        ))
    });
    let opts = || RemoteOptions {
        url: url.clone(),
        headers: remote.headers.clone().unwrap_or_default(),
        provider: provider.clone(),
        http: env.http.clone(),
    };
    let timeout = env
        .startup_timeouts
        .get(key)
        .copied()
        .or(remote.timeout)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    let mut last: Option<CreateResult> = None;
    for kind in ["StreamableHTTP", "SSE"] {
        let (attempt, auth_transport) = match kind {
            "StreamableHTTP" => {
                let (t, rx) = StreamableTransport::new(opts());
                let t = Arc::new(t);
                let r = connect_transport(t.clone(), rx, env.directory, timeout).await;
                (r, AuthTransport::Streamable(t))
            }
            _ => {
                let (t, rx) = LegacySseTransport::new(opts());
                let t = Arc::new(t);
                let started =
                    match tokio::time::timeout(Duration::from_millis(timeout), t.start()).await {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(RpcError::Transport(e)),
                        Err(_) => Err(RpcError::Other(format!(
                            "Operation timed out after {timeout}ms"
                        ))),
                    };
                let r = match started {
                    Ok(()) => connect_transport(t.clone(), rx, env.directory, timeout).await,
                    Err(e) => {
                        t.close().await;
                        Err(e)
                    }
                };
                (r, AuthTransport::Sse(t))
            }
        };
        let error = match attempt {
            Ok(client) => return connected(client),
            Err(e) => e,
        };
        let message = error.to_string();
        let unauthorized = matches!(&error, RpcError::Transport(t) if t.is_unauthorized());
        let is_auth = unauthorized || (provider.is_some() && message.contains("OAuth"));
        if is_auth {
            if message.contains("registration") || message.contains("client_id") {
                let mut r = CreateResult::status(Status::NeedsClientRegistration {
                    error: "Server does not support dynamic client registration. Please provide clientId in config.".into(),
                });
                r.toast = Some(format!(
                    "Server \"{key}\" requires a pre-registered client ID. Add clientId to your config."
                ));
                return r;
            }
            let mut r = CreateResult::status(Status::NeedsAuth);
            r.pending = Some(PendingAuth {
                transport: auth_transport,
                provider: None,
            });
            r.toast = Some(format!(
                "Server \"{key}\" requires authentication. Run: opencode mcp auth {key}"
            ));
            return r;
        }
        tracing::debug!(key, transport = kind, error = %message, "MCP remote transport failed");
        last = Some(CreateResult::status(Status::Failed { error: message }));
    }
    last.unwrap_or_else(|| {
        CreateResult::status(Status::Failed {
            error: "Unknown error".into(),
        })
    })
}
