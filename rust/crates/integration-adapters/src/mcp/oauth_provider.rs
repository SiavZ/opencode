//! Purpose: OAuth client provider for one MCP server, port of oauth-provider.ts: redirect URL,
//! client metadata, persisted vs pending credentials, PKCE verifier and state persistence.
//! Scope: credential reads/writes only. The discovery/token protocol lives in oauth.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- serde_json::{Value, json}
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/oauth.rs
//! `-- integration-adapters/src/mcp/service.rs
//! Invariants: persistent mode reads credentials only via exact-URL lookup; pending mode keeps
//! tokens and registered client in memory until `commit`; config clientId always wins.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::sync::Mutex;

use opencode_integration_api::mcp::{OAUTH_CALLBACK_PATH, OAUTH_CALLBACK_PORT};
use serde_json::{Value, json};

use super::auth_store::{ClientInfo, Entry, McpAuthStore, Tokens};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scope: Option<String>,
    pub callback_port: Option<u16>,
    pub redirect_uri: Option<String>,
}

/// `OAuthClientInformation(Full)`: what the token endpoint authenticates with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientInformation {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_id_issued_at: Option<f64>,
    pub client_secret_expires_at: Option<f64>,
    pub token_endpoint_auth_method: Option<String>,
}

/// `OAuthTokens` as returned by the token endpoint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<f64>,
    pub scope: Option<String>,
}

#[derive(Debug, Default)]
struct Pending {
    client_info: Option<ClientInformation>,
    tokens: Option<OAuthTokens>,
}

pub struct OAuthProvider {
    pub name: String,
    pub server_url: String,
    pub config: ProviderConfig,
    store: McpAuthStore,
    pending: Option<Mutex<Pending>>,
    redirect: Mutex<Option<String>>,
}

pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl OAuthProvider {
    /// `McpOAuthProvider`: credentials persist straight to the store.
    pub fn persistent(
        name: &str,
        server_url: &str,
        config: ProviderConfig,
        store: McpAuthStore,
    ) -> Self {
        Self::build(name, server_url, config, store, None)
    }

    /// `McpOAuthPendingProvider`: tokens and registration stay in memory until `commit`.
    pub fn pending(
        name: &str,
        server_url: &str,
        config: ProviderConfig,
        store: McpAuthStore,
    ) -> Self {
        Self::build(name, server_url, config, store, Some(Mutex::default()))
    }

    fn build(
        name: &str,
        url: &str,
        config: ProviderConfig,
        store: McpAuthStore,
        pending: Option<Mutex<Pending>>,
    ) -> Self {
        Self {
            name: name.into(),
            server_url: url.into(),
            config,
            store,
            pending,
            redirect: Mutex::new(None),
        }
    }

    pub fn redirect_url(&self) -> String {
        if let Some(uri) = &self.config.redirect_uri {
            return uri.clone();
        }
        let port = self.config.callback_port.unwrap_or(OAUTH_CALLBACK_PORT);
        format!("http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}")
    }

    /// Client metadata, key order as in the baseline object literal.
    pub fn client_metadata(&self) -> Value {
        let mut m = json!({
            "redirect_uris": [self.redirect_url()],
            "client_name": "OpenCode",
            "client_uri": "https://opencode.ai",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method":
                if self.config.client_secret.is_some() { "client_secret_post" } else { "none" },
        });
        if let Some(scope) = &self.config.scope {
            m["scope"] = json!(scope);
        }
        m
    }

    fn configured_client(&self) -> Option<ClientInformation> {
        self.config.client_id.as_ref().map(|id| ClientInformation {
            client_id: id.clone(),
            client_secret: self.config.client_secret.clone(),
            ..Default::default()
        })
    }

    pub async fn client_information(&self) -> Option<ClientInformation> {
        if let Some(c) = self.configured_client() {
            return Some(c);
        }
        if let Some(p) = &self.pending {
            return lock(p).client_info.clone();
        }
        let info = self
            .store
            .get_for_url(&self.name, &self.server_url)
            .await?
            .client_info?;
        if info
            .client_secret_expires_at
            .is_some_and(|t| t != 0.0 && t < now_secs())
        {
            return None;
        }
        Some(ClientInformation {
            client_id: info.client_id,
            client_secret: info.client_secret,
            ..Default::default()
        })
    }

    pub async fn save_client_information(&self, info: ClientInformation) -> std::io::Result<()> {
        if let Some(p) = &self.pending {
            lock(p).client_info = Some(info);
            return Ok(());
        }
        self.store
            .update_client_info(&self.name, to_store_client(&info), Some(&self.server_url))
            .await
    }

    pub async fn tokens(&self) -> Option<OAuthTokens> {
        if let Some(p) = &self.pending {
            return lock(p).tokens.clone();
        }
        let t = self
            .store
            .get_for_url(&self.name, &self.server_url)
            .await?
            .tokens?;
        Some(OAuthTokens {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
            expires_in: t
                .expires_at
                .filter(|e| *e != 0.0)
                .map(|e| (e - now_secs()).floor().max(0.0)),
            scope: t.scope,
        })
    }

    pub async fn save_tokens(&self, tokens: OAuthTokens) -> std::io::Result<()> {
        if let Some(p) = &self.pending {
            lock(p).tokens = Some(tokens);
            return Ok(());
        }
        self.store
            .update_tokens(&self.name, to_store_tokens(&tokens), Some(&self.server_url))
            .await
    }

    /// Validate the scheme and capture the URL (`onRedirect`).
    pub fn redirect_to_authorization(&self, url: &url::Url) -> Result<(), String> {
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "MCP server \"{}\" returned a {}: authorization URL; only http and https are supported",
                self.server_url,
                url.scheme()
            ));
        }
        *lock(&self.redirect) = Some(url.to_string());
        Ok(())
    }

    pub fn captured_redirect(&self) -> Option<String> {
        lock(&self.redirect).clone()
    }

    pub async fn save_code_verifier(&self, verifier: &str) -> std::io::Result<()> {
        self.store.update_code_verifier(&self.name, verifier).await
    }

    pub async fn code_verifier(&self) -> Result<String, String> {
        self.store
            .get(&self.name)
            .await
            .and_then(|e| e.code_verifier)
            .ok_or_else(|| format!("No code verifier saved for MCP server: {}", self.name))
    }

    /// Reads the stored state or generates and persists a new one (the SDK calls it as a generator).
    pub async fn state(&self) -> std::io::Result<String> {
        if let Some(s) = self.store.get_oauth_state(&self.name).await {
            return Ok(s);
        }
        let state = random_hex(32);
        self.store.update_oauth_state(&self.name, &state).await?;
        Ok(state)
    }

    pub async fn invalidate_credentials(&self, kind: Invalidate) -> std::io::Result<()> {
        if let Some(p) = &self.pending {
            let mut p = lock(p);
            if matches!(kind, Invalidate::All | Invalidate::Client) {
                p.client_info = None;
            }
            if matches!(kind, Invalidate::All | Invalidate::Tokens) {
                p.tokens = None;
            }
            return Ok(());
        }
        let Some(mut entry) = self.store.get(&self.name).await else {
            return Ok(());
        };
        match kind {
            Invalidate::All => return self.store.remove(&self.name).await,
            Invalidate::Client => entry.client_info = None,
            Invalidate::Tokens => entry.tokens = None,
        }
        self.store.set(&self.name, entry, None).await
    }

    /// Pending mode: persist tokens (+ registered client unless configured) bound to the URL.
    pub async fn commit(&self) -> std::io::Result<()> {
        let Some(p) = &self.pending else {
            return Ok(());
        };
        let (tokens, client) = {
            let p = lock(p);
            (p.tokens.clone(), p.client_info.clone())
        };
        let Some(tokens) = tokens else { return Ok(()) };
        let entry = Entry {
            tokens: Some(to_store_tokens(&tokens)),
            client_info: client
                .filter(|_| self.config.client_id.is_none())
                .map(|c| to_store_client(&c)),
            ..Default::default()
        };
        self.store
            .set(&self.name, entry, Some(&self.server_url))
            .await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalidate {
    All,
    Client,
    Tokens,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn to_store_tokens(t: &OAuthTokens) -> Tokens {
    Tokens {
        access_token: t.access_token.clone(),
        refresh_token: t.refresh_token.clone(),
        expires_at: t.expires_in.filter(|e| *e != 0.0).map(|e| now_secs() + e),
        scope: t.scope.clone(),
    }
}

fn to_store_client(c: &ClientInformation) -> ClientInfo {
    ClientInfo {
        client_id: c.client_id.clone(),
        client_secret: c.client_secret.clone(),
        client_id_issued_at: c.client_id_issued_at,
        client_secret_expires_at: c.client_secret_expires_at,
    }
}

/// `crypto.getRandomValues(new Uint8Array(n))` rendered as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).unwrap_or_else(|_| {
        let seed = now_secs().to_bits();
        buf.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = (seed >> (i % 8)) as u8);
    });
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Baseline `parseRedirectUri`: port and path for the callback server, defaults on parse errors.
pub fn parse_redirect_uri(uri: Option<&str>) -> (u16, String) {
    let default = (OAUTH_CALLBACK_PORT, OAUTH_CALLBACK_PATH.to_string());
    let Some(uri) = uri else { return default };
    let Ok(url) = url::Url::parse(uri) else {
        return default;
    };
    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let path = if url.path().is_empty() {
        OAUTH_CALLBACK_PATH.to_string()
    } else {
        url.path().to_string()
    };
    (port, path)
}
