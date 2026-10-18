//! Purpose: MCP OAuth 2.1 client orchestration, port of the SDK `auth()` the baseline relies on:
//! discovery, client registration, code exchange, refresh, or a PKCE authorization redirect.
//! Scope: stateless functions over an `OAuthProvider` and a reqwest client.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth_discovery.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! `-- integration-adapters/src/mcp/oauth_token.rs
//! External:
//! `-- reqwest::Client
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/oauth_discovery.rs
//! |-- integration-adapters/src/mcp/oauth_token.rs
//! `-- integration-adapters/src/mcp/remote.rs
//! Invariants: error messages match the SDK strings the baseline classifies on ("registration",
//! "client_id", "OAuth"); invalid_client/unauthorized_client retry once after invalidating all,
//! invalid_grant retries once after invalidating tokens.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::collections::BTreeMap;

use serde_json::Value;

use super::oauth_discovery::{
    determine_scope, discover_as_metadata, discover_protected_resource, parse_url, select_resource,
};
use super::oauth_provider::{Invalidate, OAuthProvider, OAuthTokens};
use super::oauth_token::{authorization_url, register_client, token_request};
pub use super::oauth_token::{pkce, www_authenticate_params};

pub(super) const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthResult {
    Authorized,
    Redirect,
}

/// SDK `OAuthError` (with `code`) or a plain `Error` (`code: None`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct AuthError {
    pub code: Option<String>,
    pub message: String,
}

impl AuthError {
    pub(super) fn plain(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

pub struct AuthRequest<'a> {
    pub http: &'a reqwest::Client,
    pub headers: &'a BTreeMap<String, String>,
    pub server_url: &'a str,
    pub authorization_code: Option<&'a str>,
    pub resource_metadata_url: Option<&'a str>,
    pub scope: Option<&'a str>,
}

/// SDK `auth()`: one retry after credential invalidation for recoverable OAuth errors.
pub async fn auth(
    provider: &OAuthProvider,
    req: &AuthRequest<'_>,
) -> Result<AuthResult, AuthError> {
    match auth_internal(provider, req).await {
        Err(e)
            if matches!(
                e.code.as_deref(),
                Some("invalid_client" | "unauthorized_client")
            ) =>
        {
            invalidate(provider, Invalidate::All).await?;
            auth_internal(provider, req).await
        }
        Err(e) if e.code.as_deref() == Some("invalid_grant") => {
            invalidate(provider, Invalidate::Tokens).await?;
            auth_internal(provider, req).await
        }
        other => other,
    }
}

async fn invalidate(provider: &OAuthProvider, kind: Invalidate) -> Result<(), AuthError> {
    provider
        .invalidate_credentials(kind)
        .await
        .map_err(|e| AuthError::plain(e.to_string()))
}

async fn auth_internal(
    provider: &OAuthProvider,
    req: &AuthRequest<'_>,
) -> Result<AuthResult, AuthError> {
    let server = parse_url(req.server_url)?;
    let resource_metadata = discover_protected_resource(req, &server).await.ok();
    let as_url = resource_metadata
        .as_ref()
        .and_then(|m| m.get("authorization_servers"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}/", server.origin().ascii_serialization()));
    let as_url = parse_url(&as_url)?;
    let metadata = discover_as_metadata(req, &as_url).await?;
    let resource = select_resource(&server, resource_metadata.as_ref())?;
    let scope = determine_scope(
        req.scope,
        resource_metadata.as_ref(),
        metadata.as_ref(),
        provider,
    );

    let client = match provider.client_information().await {
        Some(c) => c,
        None => {
            if req.authorization_code.is_some() {
                return Err(AuthError::plain(
                    "Existing OAuth client information is required when exchanging an authorization code",
                ));
            }
            let info = register_client(req, &as_url, metadata.as_ref(), provider, scope.as_deref())
                .await?;
            provider
                .save_client_information(info.clone())
                .await
                .map_err(|e| AuthError::plain(e.to_string()))?;
            info
        }
    };

    if let Some(code) = req.authorization_code {
        let verifier = provider.code_verifier().await.map_err(AuthError::plain)?;
        let mut params = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".into(), code.to_string()),
            ("code_verifier".into(), verifier),
            ("redirect_uri".into(), provider.redirect_url()),
        ];
        let tokens = token_request(
            req,
            &as_url,
            metadata.as_ref(),
            &mut params,
            &client,
            resource.as_ref(),
        )
        .await?;
        save(provider, tokens).await?;
        return Ok(AuthResult::Authorized);
    }

    if let Some(refresh) = provider.tokens().await.and_then(|t| t.refresh_token) {
        let mut params = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("refresh_token".into(), refresh.clone()),
        ];
        match token_request(
            req,
            &as_url,
            metadata.as_ref(),
            &mut params,
            &client,
            resource.as_ref(),
        )
        .await
        {
            Ok(mut tokens) => {
                tokens.refresh_token.get_or_insert(refresh);
                save(provider, tokens).await?;
                return Ok(AuthResult::Authorized);
            }
            Err(e) if e.code.as_deref().is_some_and(|c| c != "server_error") => return Err(e),
            Err(_) => {}
        }
    }

    let state = provider
        .state()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    let (url, verifier) = authorization_url(
        &as_url,
        metadata.as_ref(),
        &client,
        provider,
        &state,
        scope.as_deref(),
        resource.as_ref(),
    )?;
    provider
        .save_code_verifier(&verifier)
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    provider
        .redirect_to_authorization(&url)
        .map_err(AuthError::plain)?;
    Ok(AuthResult::Redirect)
}

async fn save(provider: &OAuthProvider, tokens: OAuthTokens) -> Result<(), AuthError> {
    provider
        .save_tokens(tokens)
        .await
        .map_err(|e| AuthError::plain(e.to_string()))
}
