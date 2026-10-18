//! Purpose: framework-agnostic ports of the hosted GitHub token routes
//! (`packages/function/src/api.ts:259-387`): `POST /exchange_github_app_token` (OIDC),
//! `POST /exchange_github_app_token_with_pat` and `GET /get_github_app_installation`. Each
//! takes a plain request value and returns status/content-type/body with the baseline JSON
//! and error strings. Phase 13 mounts these in the hosted service. Not-matching paths return
//! `None` so the composing router keeps its own fallback.
//! Scope: request.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/app.rs
//! |-- ecosystem/src/github/auth/mod.rs
//! `-- ecosystem/src/github/auth/oidc.rs
//! External:
//! `-- serde_json::{json, Value}
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/bridge.rs
//! |-- ecosystem/src/github/auth/mod.rs
//! `-- ecosystem/tests/github_auth_exchange.rs
//! Invariants: verification failures never reveal why (baseline 403 body is fixed), tokens
//! and request bodies are never logged.
//! Contract tests: ecosystem/tests/github_auth_exchange.rs

use serde_json::{Value, json};

use super::AuthError;
use super::app::{AppCredentials, GitHubApi};
use super::oidc::OidcVerifier;

/// Minimal HTTP request view. Header names are matched case-insensitively.
#[derive(Debug, Clone, Default)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `header("Authorization")?.replace(/^Bearer /, "")`, empty treated as missing.
    pub fn bearer(&self) -> Option<&str> {
        let value = self.header("authorization")?;
        let token = value.strip_prefix("Bearer ").unwrap_or(value);
        (!token.is_empty()).then_some(token)
    }
}

/// Response with exact status, content type and body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl HttpResponse {
    /// Headers for framework adapters. No token-bearing headers are emitted.
    pub fn headers(&self) -> [(&'static str, &'static str); 1] {
        [("content-type", self.content_type)]
    }

    /// Hono `c.json(value, { status })`.
    pub fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string(),
        }
    }

    /// Hono's default error handler response for an uncaught exception.
    pub fn internal_error() -> Self {
        Self {
            status: 500,
            content_type: "text/plain; charset=UTF-8",
            body: "Internal Server Error".into(),
        }
    }

    pub fn json_body(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// Dependencies of the exchange routes. `now` returns unix seconds (injectable for tests).
pub struct ExchangeService {
    pub verifier: OidcVerifier,
    pub api: GitHubApi,
    pub app: AppCredentials,
    pub now: fn() -> u64,
}

impl std::fmt::Debug for ExchangeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeService")
            .field("verifier", &self.verifier)
            .field("api", &self.api)
            .field("app", &self.app)
            .finish()
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ExchangeService {
    pub fn new(verifier: OidcVerifier, api: GitHubApi, app: AppCredentials) -> Self {
        Self {
            verifier,
            api,
            app,
            now: unix_now,
        }
    }

    /// Dispatch by method and path. `None` means the route is not owned here.
    pub async fn handle(&self, request: &HttpRequest) -> Option<HttpResponse> {
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/exchange_github_app_token") => Some(self.exchange_oidc(request).await),
            ("POST", "/exchange_github_app_token_with_pat") => {
                Some(self.exchange_pat(request).await)
            }
            ("GET", "/get_github_app_installation") => Some(self.installation(request).await),
            _ => None,
        }
    }

    /// `api.ts:262-308`.
    pub async fn exchange_oidc(&self, request: &HttpRequest) -> HttpResponse {
        let Some(token) = request.bearer() else {
            return HttpResponse::json(401, json!({ "error": "Authorization header is required" }));
        };
        let repository = match self.verifier.verify_repository(token).await {
            Ok(repository) => repository,
            Err(err) => {
                tracing::warn!(kind = kind(&err), "OIDC token verification failed");
                return HttpResponse::json(403, json!({ "error": "Invalid or expired token" }));
            }
        };
        match self
            .api
            .app_installation_token(&self.app, (self.now)(), &repository.owner, &repository.repo)
            .await
        {
            Ok(token) => HttpResponse::json(200, json!({ "token": token })),
            Err(err) => {
                tracing::warn!(kind = kind(&err), "GitHub App token exchange failed");
                HttpResponse::json(
                    502,
                    json!({ "error": format!(
                        "Failed to exchange GitHub App token for {}/{}",
                        repository.owner, repository.repo
                    ) }),
                )
            }
        }
    }

    /// `api.ts:312-358`. Invalid JSON bodies throw before the baseline `try`, so they reach
    /// Hono's default 500 handler.
    pub async fn exchange_pat(&self, request: &HttpRequest) -> HttpResponse {
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            return HttpResponse::internal_error();
        };
        // `body.owner` is outside the baseline try block and throws on null.
        if body.is_null() {
            return HttpResponse::internal_error();
        }
        let owner = js_string(body.get("owner"));
        let repo = js_string(body.get("repo"));
        match self.pat_flow(request.bearer(), &owner, &repo).await {
            Ok(token) => HttpResponse::json(200, json!({ "token": token })),
            Err(err) => HttpResponse::json(401, json!({ "error": err.to_string() })),
        }
    }

    async fn pat_flow(
        &self,
        token: Option<&str>,
        owner: &str,
        repo: &str,
    ) -> Result<String, AuthError> {
        let token =
            token.ok_or_else(|| AuthError::Invalid("Authorization header is required".into()))?;
        let permissions = self.api.repo_permissions(token, owner, repo).await?;
        if !permissions.admin && !permissions.push && !permissions.maintain {
            return Err(AuthError::PermissionDenied(
                "User does not have write permissions".into(),
            ));
        }
        self.api
            .app_installation_token(&self.app, (self.now)(), owner, repo)
            .await
    }

    /// `api.ts:362-386`. A "Not Found" lookup error answers `{}` (the baseline serializes
    /// `{ installation: undefined }`), any other failure is an uncaught 500.
    pub async fn installation(&self, request: &HttpRequest) -> HttpResponse {
        let owner = request.query("owner").unwrap_or("undefined");
        let repo = request.query("repo").unwrap_or("undefined");
        let Ok(jwt) = self.app.app_jwt((self.now)()) else {
            return HttpResponse::internal_error();
        };
        match self.api.repo_installation(&jwt, owner, repo).await {
            Ok(installation) => HttpResponse::json(200, json!({ "installation": installation })),
            Err(err) if err.to_string().contains("Not Found") => HttpResponse::json(200, json!({})),
            Err(err) => {
                tracing::warn!(kind = kind(&err), "GitHub App installation lookup failed");
                HttpResponse::internal_error()
            }
        }
    }
}

/// Template-literal coercion of an optional JSON value (`${body.owner}`).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".into(),
        Some(other) => other.to_string(),
    }
}

fn kind(err: &AuthError) -> &'static str {
    match err {
        AuthError::Invalid(_) => "invalid",
        AuthError::PermissionDenied(_) => "permission_denied",
        AuthError::Upstream { .. } => "upstream",
        AuthError::Transport(_) => "transport",
        AuthError::Config(_) => "config",
    }
}
