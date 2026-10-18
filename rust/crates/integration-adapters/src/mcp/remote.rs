//! Purpose: shared pieces of the remote transports: header assembly (bearer token, session,
//! protocol version, then user headers which win), the 401/403 OAuth retry decision, and a
//! task set that is aborted and joined on close.
//! Scope: helpers only, used by streamable.rs and legacy_sse.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! `-- reqwest::{Client, RequestBuilder}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! `-- integration-adapters/src/mcp/streamable.rs
//! Invariants: user headers override generated ones (SDK `new Headers({...headers, ...extra})`);
//! an auth attempt that does not end `Authorized` surfaces `TransportError::Unauthorized`.
//! Contract tests: integration-adapters/tests/mcp_http.rs

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::oauth::{AuthRequest, AuthResult, auth, www_authenticate_params};
use super::oauth_provider::OAuthProvider;
use super::transport::{AuthHints, TransportError};

pub struct RemoteOptions {
    pub url: url::Url,
    pub headers: BTreeMap<String, String>,
    pub provider: Option<Arc<OAuthProvider>>,
    pub http: reqwest::Client,
}

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub async fn apply_headers(
    mut b: reqwest::RequestBuilder,
    opts: &RemoteOptions,
    generated: &[(&str, Option<String>)],
) -> reqwest::RequestBuilder {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(p) = &opts.provider
        && let Some(t) = p.tokens().await
    {
        map.insert("authorization".into(), format!("Bearer {}", t.access_token));
    }
    for (k, v) in generated {
        if let Some(v) = v {
            map.insert(k.to_ascii_lowercase(), v.clone());
        }
    }
    for (k, v) in &opts.headers {
        map.insert(k.to_ascii_lowercase(), v.clone());
    }
    for (k, v) in map {
        b = b.header(k, v);
    }
    b
}

/// Record hints from a `WWW-Authenticate` header (SDK `extractWWWAuthenticateParams`).
pub fn record_hints(hints: &Mutex<AuthHints>, header: Option<&str>) -> Option<String> {
    let (rm, scope, error) = www_authenticate_params(header);
    let mut h = lock(hints);
    h.resource_metadata_url = rm;
    h.scope = scope;
    error
}

/// Run SDK `auth()` for a transport and map the outcome to a transport error.
pub async fn authorize(
    opts: &RemoteOptions,
    hints: &Mutex<AuthHints>,
    code: Option<&str>,
) -> Result<(), TransportError> {
    let Some(provider) = &opts.provider else {
        return Err(TransportError::Unauthorized("No auth provider".into()));
    };
    let h = lock(hints).clone();
    let req = AuthRequest {
        http: &opts.http,
        headers: &opts.headers,
        server_url: opts.url.as_str(),
        authorization_code: code,
        resource_metadata_url: h.resource_metadata_url.as_deref(),
        scope: h.scope.as_deref(),
    };
    match auth(provider, &req).await {
        Ok(AuthResult::Authorized) => Ok(()),
        Ok(AuthResult::Redirect) if code.is_some() => {
            Err(TransportError::Unauthorized("Failed to authorize".into()))
        }
        Ok(AuthResult::Redirect) => Err(TransportError::Unauthorized("Unauthorized".into())),
        // SDK auth errors propagate as plain errors, classified later by message text.
        Err(e) => Err(TransportError::Failed(e.to_string())),
    }
}

#[derive(Default)]
pub struct Tasks(Mutex<Vec<tokio::task::JoinHandle<()>>>);

impl Tasks {
    pub fn push(&self, h: tokio::task::JoinHandle<()>) {
        let mut v = lock(&self.0);
        v.retain(|t| !t.is_finished());
        v.push(h);
    }

    pub async fn join_all(&self) {
        let all: Vec<_> = lock(&self.0).drain(..).collect();
        for t in all {
            t.abort();
            let _ = t.await;
        }
    }
}
