//! Purpose: MCP OAuth flows on the service, port of `startAuth`, `authenticate`, `finishAuth`,
//! `removeAuth` and `getAuthStatus` (baseline index.ts:806-970).
//! Scope: methods on `McpServiceImpl`; protocol in oauth.rs, persistence in auth_store.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-adapters/src/mcp/streamable.rs
//! |-- integration-api/src/error.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/service.rs
//! `-- integration-adapters/src/mcp/service_api.rs
//! Invariants: the oauth state is generated (32 random bytes hex) and stored before the
//! authorization URL is built; the callback wait is registered before the browser opens; a
//! returned code is only exchanged when the stored state still matches.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::sync::Arc;

use opencode_integration_api::error::IntegrationError;
use opencode_integration_api::mcp::{
    AuthStatus, BrowserOpenFailedProps, McpEvent, OAUTH_CALLBACK_PATH, OAuthSetting, ServerConfig,
    StartedAuth, Status,
};

use super::auth_store::Field;
use super::catalog;
use super::client::McpClient;
use super::connect::{AuthTransport, PendingAuth, provider_config};
use super::jsonrpc::RpcError;
use super::oauth_provider::{OAuthProvider, now_secs, random_hex};
use super::remote::RemoteOptions;
use super::service::{McpServiceImpl, enable, lock};
use super::streamable::StreamableTransport;

pub struct StartResult {
    pub started: StartedAuth,
    /// Set when the server accepted the connection without a redirect.
    pub client: Option<McpClient>,
}

fn failed(name: &str, message: impl Into<String>) -> IntegrationError {
    IntegrationError::Failed {
        name: name.into(),
        message: message.into(),
    }
}

impl McpServiceImpl {
    pub(super) async fn start_auth_flow(
        &self,
        name: &str,
    ) -> Result<StartResult, IntegrationError> {
        let ServerConfig::Remote(remote) = self.require_config(name)? else {
            return Err(failed(
                name,
                format!("MCP server {name} is not a remote server"),
            ));
        };
        if matches!(remote.oauth, Some(OAuthSetting::Disabled(_))) {
            return Err(failed(
                name,
                format!("MCP server {name} has OAuth explicitly disabled"),
            ));
        }
        let Ok(url) = url::Url::parse(&remote.url) else {
            return Err(failed(name, format!("Invalid MCP URL for \"{name}\"")));
        };
        let mut config = provider_config(&remote);
        let redirect = config.redirect_uri.clone().or_else(|| {
            config
                .callback_port
                .map(|p| format!("http://127.0.0.1:{p}{OAUTH_CALLBACK_PATH}"))
        });
        self.0
            .callback
            .ensure_running(redirect.as_deref())
            .await
            .map_err(|e| failed(name, e.to_string()))?;
        let oauth_state = random_hex(32);
        self.0
            .store
            .update_oauth_state(name, &oauth_state)
            .await
            .map_err(|e| failed(name, e.to_string()))?;
        config.redirect_uri = redirect;
        config.callback_port = None;
        let provider = Arc::new(OAuthProvider::pending(
            name,
            &remote.url,
            config,
            self.0.store.clone(),
        ));
        let (transport, rx) = StreamableTransport::new(RemoteOptions {
            url,
            headers: remote.headers.clone().unwrap_or_default(),
            provider: Some(provider.clone()),
            http: self.0.http.clone(),
        });
        let transport = Arc::new(transport);
        // Baseline startAuth does not wrap the connect in the server timeout (SDK default only).
        let attempt = McpClient::connect(transport.clone(), rx, &self.0.directory).await;
        match attempt {
            Ok(client) => {
                provider
                    .commit()
                    .await
                    .map_err(|e| failed(name, e.to_string()))?;
                Ok(StartResult {
                    started: StartedAuth {
                        authorization_url: String::new(),
                        oauth_state,
                    },
                    client: Some(client),
                })
            }
            Err(RpcError::Transport(e))
                if e.is_unauthorized() && provider.captured_redirect().is_some() =>
            {
                let authorization_url = provider.captured_redirect().unwrap_or_default();
                lock(&self.0.state).pending.insert(
                    name.into(),
                    PendingAuth {
                        transport: AuthTransport::Streamable(transport),
                        provider: Some(provider),
                    },
                );
                Ok(StartResult {
                    started: StartedAuth {
                        authorization_url,
                        oauth_state,
                    },
                    client: None,
                })
            }
            Err(e) => Err(failed(name, e.to_string())),
        }
    }

    /// `authenticate(name, onAuthorization)`.
    pub async fn authenticate_flow(
        &self,
        name: &str,
        on_authorization: Option<&(dyn Fn(&str) + Send + Sync)>,
    ) -> Result<Status, IntegrationError> {
        self.ready().await;
        let result = self.start_auth_flow(name).await?;
        if result.started.authorization_url.is_empty() {
            let client = result.client.map(Arc::new);
            let config = match self.require_config(name) {
                Ok(c) => c,
                Err(e) => {
                    if let Some(c) = &client {
                        c.close().await;
                    }
                    return Err(e);
                }
            };
            let listed = match &client {
                Some(c) if c.has_capability("tools") => catalog::defs(c, config.timeout_ms()).await,
                Some(_) => Some(Vec::new()),
                None => None,
            };
            let (Some(client), Some(listed)) = (client.clone(), listed) else {
                if let Some(c) = client {
                    c.close().await;
                }
                return Ok(Status::Failed {
                    error: "Failed to get tools".into(),
                });
            };
            let _ = self.0.store.clear(name, Field::OAuthState).await;
            let instructions = client.instructions();
            return Ok(self
                .store_client(name, client, listed, instructions, config.timeout_ms())
                .await);
        }
        let url = result.started.authorization_url.clone();
        let wait = self
            .0
            .callback
            .register(&result.started.oauth_state, Some(name));
        if let Some(cb) = on_authorization {
            cb(&url);
        }
        if self.0.browser.open(&url).await.is_err() {
            self.0.events.publish(McpEvent::BrowserOpenFailed {
                properties: BrowserOpenFailedProps {
                    mcp_name: name.into(),
                    url: url.clone(),
                },
            });
        }
        let code = wait.wait().await.map_err(|e| failed(name, e))?;
        let stored = self.0.store.get_oauth_state(name).await;
        let _ = self.0.store.clear(name, Field::OAuthState).await;
        if stored.as_deref() != Some(result.started.oauth_state.as_str()) {
            return Err(failed(name, "OAuth state mismatch - potential CSRF attack"));
        }
        self.finish_auth_flow(name, &code).await
    }

    pub(super) async fn finish_auth_flow(
        &self,
        name: &str,
        code: &str,
    ) -> Result<Status, IntegrationError> {
        self.require_config(name)?;
        self.ready().await;
        let pending = {
            let s = lock(&self.0.state);
            s.pending
                .get(name)
                .map(|p| (p.transport.clone(), p.provider.clone()))
        };
        let Some((transport, provider)) = pending else {
            return Err(failed(
                name,
                format!("No pending OAuth flow for MCP server: {name}"),
            ));
        };
        if let Err(e) = transport.finish_auth(code).await {
            return Ok(Status::Failed {
                error: format!("OAuth completion failed: {e}"),
            });
        }
        if let Some(p) = provider {
            p.commit().await.map_err(|e| failed(name, e.to_string()))?;
        }
        let _ = self.0.store.clear(name, Field::CodeVerifier).await;
        lock(&self.0.state).pending.remove(name);
        let config = self.require_config(name)?;
        Ok(self.create_and_store(name, enable(config)).await)
    }

    pub(super) async fn remove_auth_flow(&self, name: &str) -> Result<(), IntegrationError> {
        self.0
            .store
            .remove(name)
            .await
            .map_err(|e| failed(name, e.to_string()))?;
        self.0.callback.cancel_pending(name);
        lock(&self.0.state).pending.remove(name);
        Ok(())
    }

    pub(super) async fn auth_status_flow(&self, name: &str) -> AuthStatus {
        let Some(ServerConfig::Remote(remote)) = self.config_for(name) else {
            return AuthStatus::NotAuthenticated;
        };
        let Some(tokens) = self
            .0
            .store
            .get_for_url(name, &remote.url)
            .await
            .and_then(|e| e.tokens)
        else {
            return AuthStatus::NotAuthenticated;
        };
        if tokens
            .expires_at
            .is_some_and(|t| t != 0.0 && t < now_secs())
        {
            return AuthStatus::Expired;
        }
        AuthStatus::Authenticated
    }
}
