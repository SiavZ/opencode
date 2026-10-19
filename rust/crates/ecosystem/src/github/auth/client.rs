//! Purpose: Action-side GitHub token lifecycle: choose `GITHUB_TOKEN` (`use_github_token`) or
//! fetch an Actions OIDC ID token and exchange it (or a `github_pat_` token in local/mock runs)
//! at the opencode exchange API, then revoke the App installation token on exit.
//! Baselines: `packages/opencode/src/cli/cmd/github.handler.ts:411,474-484,686-700,984-1020,
//! 655-660,1593-1604`, root host `github/index.ts:334-348,369-401,1060-1072` and
//! `@actions/core` 1.11.1 `OidcClient.getIDToken` (URL + `&audience=`, bearer request token,
//! `.value` field, retries 502/503/504 up to 10 times with exponential backoff).
//! Does not configure Git credentials (github/run owns extraheader/identity).
//! Scope: one Action run. The returned `AcquiredToken` records whether it must be revoked.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/app.rs
//! `-- ecosystem/src/github/auth/mod.rs
//! External:
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- str::to_string
//! |-- tokio::time::sleep
//! |-- tracing::error
//! |-- tracing::warn
//! `-- url::form_urlencoded::byte_serialize
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/github/auth/mod.rs
//! `-- ecosystem/tests/github_auth_client.rs
//! Invariants: tokens are never included in errors or logs, only App tokens obtained by
//! exchange are revoked, the GITHUB_TOKEN path never revokes.
//! Contract tests: ecosystem/tests/github_auth_client.rs

use std::time::Duration;

use serde_json::{Value, json};

use super::AuthError;
use super::app::GITHUB_API;

pub const DEFAULT_OIDC_BASE_URL: &str = "https://api.opencode.ai";
pub const OIDC_AUDIENCE: &str = "opencode-github-action";
const RETRY_CODES: [u16; 3] = [502, 503, 504];
const MAX_RETRIES: u32 = 10;

/// Inputs read from the Action environment. Construct directly in tests.
#[derive(Clone, Default)]
pub struct TokenEnv {
    /// `USE_GITHUB_TOKEN` raw value.
    pub use_github_token: Option<String>,
    /// `GITHUB_TOKEN`.
    pub github_token: Option<String>,
    /// `OIDC_BASE_URL` raw value.
    pub oidc_base_url: Option<String>,
    /// `ACTIONS_ID_TOKEN_REQUEST_URL`.
    pub id_token_request_url: Option<String>,
    /// `ACTIONS_ID_TOKEN_REQUEST_TOKEN`.
    pub id_token_request_token: Option<String>,
}

impl std::fmt::Debug for TokenEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenEnv")
            .field("use_github_token", &self.use_github_token)
            .field(
                "github_token",
                &self.github_token.as_ref().map(|_| "<redacted>"),
            )
            .field("oidc_base_url", &self.oidc_base_url)
            .field(
                "id_token_request_url",
                &self.id_token_request_url.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl TokenEnv {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            use_github_token: var("USE_GITHUB_TOKEN"),
            github_token: var("GITHUB_TOKEN"),
            oidc_base_url: var("OIDC_BASE_URL"),
            id_token_request_url: var("ACTIONS_ID_TOKEN_REQUEST_URL"),
            id_token_request_token: var("ACTIONS_ID_TOKEN_REQUEST_TOKEN"),
        }
    }

    /// `normalizeUseGithubToken`.
    pub fn use_github_token(&self) -> Result<bool, AuthError> {
        match self.use_github_token.as_deref() {
            None | Some("") => Ok(false),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(other) => Err(AuthError::Config(format!(
                "Invalid use_github_token value: {other}. Must be a boolean."
            ))),
        }
    }

    /// `normalizeOidcBaseUrl`: default, trailing slashes removed.
    pub fn oidc_base_url(&self) -> String {
        match self.oidc_base_url.as_deref() {
            None | Some("") => DEFAULT_OIDC_BASE_URL.into(),
            Some(value) => value.trim_end_matches('/').into(),
        }
    }
}

/// Token for the run plus whether it is an exchanged App token that must be revoked.
#[derive(Clone, PartialEq, Eq)]
pub struct AcquiredToken {
    pub token: String,
    pub source: TokenSource,
}

impl std::fmt::Debug for AcquiredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcquiredToken")
            .field("token", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// Workflow `GITHUB_TOKEN`; Git config and revoke are skipped by the baseline.
    GithubToken,
    /// App installation token from the exchange API; must be revoked on exit.
    AppInstallation,
}

impl AcquiredToken {
    pub fn must_revoke(&self) -> bool {
        self.source == TokenSource::AppInstallation
    }
}

/// HTTP client for OIDC fetch, exchange and revoke. Base URLs are injectable.
#[derive(Debug, Clone)]
pub struct TokenClient {
    http: reqwest::Client,
    github_api: String,
    retry_slice: Duration,
}

impl Default for TokenClient {
    fn default() -> Self {
        Self::new(GITHUB_API)
    }
}

impl TokenClient {
    pub fn new(github_api: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            github_api: github_api.into().trim_end_matches('/').into(),
            retry_slice: Duration::from_millis(5),
        }
    }

    /// CLI run token selection (`github.handler.ts:474-484`). `local_token` is the
    /// `--token` argument used in mock runs instead of an OIDC token.
    pub async fn acquire(
        &self,
        env: &TokenEnv,
        owner: &str,
        repo: &str,
        local_token: Option<&str>,
    ) -> Result<AcquiredToken, AuthError> {
        if env.use_github_token()? {
            let token = env.github_token.clone().filter(|t| !t.is_empty()).ok_or_else(|| {
                AuthError::Config("GITHUB_TOKEN environment variable is not set. When using use_github_token, you must provide GITHUB_TOKEN.".into())
            })?;
            return Ok(AcquiredToken {
                token,
                source: TokenSource::GithubToken,
            });
        }
        let action_token = match local_token {
            Some(token) => token.to_string(),
            None => self.oidc_token(env).await.map_err(|_| {
                // Baseline logs the cause separately and throws a fixed message.
                tracing::error!("Failed to get OIDC token");
                AuthError::Config(
                    "Could not fetch an OIDC token. Make sure to add `id-token: write` to your workflow permissions.".into(),
                )
            })?,
        };
        let token = self
            .exchange(&env.oidc_base_url(), &action_token, owner, repo)
            .await?;
        Ok(AcquiredToken {
            token,
            source: TokenSource::AppInstallation,
        })
    }

    /// `core.getIDToken("opencode-github-action")`.
    pub async fn oidc_token(&self, env: &TokenEnv) -> Result<String, AuthError> {
        let request_token = env.id_token_request_token.as_deref().ok_or_else(|| {
            AuthError::Config(
                "Error message: Unable to get ACTIONS_ID_TOKEN_REQUEST_TOKEN env variable".into(),
            )
        })?;
        let base = env.id_token_request_url.as_deref().ok_or_else(|| {
            AuthError::Config(
                "Error message: Unable to get ACTIONS_ID_TOKEN_REQUEST_URL env variable".into(),
            )
        })?;
        let audience: String =
            url::form_urlencoded::byte_serialize(OIDC_AUDIENCE.as_bytes()).collect();
        let url = format!("{base}&audience={audience}");
        let mut attempt = 0;
        let response = loop {
            let response = self
                .http
                .get(&url)
                .bearer_auth(request_token)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|e| {
                    AuthError::Transport(format!(
                        "Error message: Failed to get ID Token. {}",
                        e.without_url()
                    ))
                })?;
            let status = response.status().as_u16();
            if !RETRY_CODES.contains(&status) || attempt >= MAX_RETRIES {
                break response;
            }
            attempt += 1;
            tokio::time::sleep(self.retry_slice * 2u32.pow(attempt.min(10))).await;
        };
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(AuthError::Upstream {
                status,
                message: format!("Error message: Failed to get ID Token. Error Code : {status}"),
            });
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| AuthError::Invalid(format!("Error message: {e}")))?;
        body.get("value")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                AuthError::Invalid(
                    "Error message: Response json body do not have ID Token field".into(),
                )
            })
    }

    /// `exchangeForAppToken` (`github.handler.ts:996-1020`): `github_pat_` tokens use the PAT
    /// route with an `{owner, repo}` body, everything else the OIDC route.
    pub async fn exchange(
        &self,
        base_url: &str,
        token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<String, AuthError> {
        self.exchange_inner(
            base_url,
            token,
            owner,
            repo,
            token.starts_with("github_pat_"),
            false,
        )
        .await
    }

    /// Root-host `getAccessToken`: mock mode always uses the PAT route, and errors
    /// contain the response JSON's `error` field rather than the whole response body.
    pub async fn exchange_root(
        &self,
        base_url: &str,
        token: &str,
        owner: &str,
        repo: &str,
        mock: bool,
    ) -> Result<String, AuthError> {
        self.exchange_inner(base_url, token, owner, repo, mock, true)
            .await
    }

    async fn exchange_inner(
        &self,
        base_url: &str,
        token: &str,
        owner: &str,
        repo: &str,
        pat: bool,
        root: bool,
    ) -> Result<String, AuthError> {
        let request = if pat {
            self.http
                .post(format!("{base_url}/exchange_github_app_token_with_pat"))
                .body(json!({ "owner": owner, "repo": repo }).to_string())
        } else {
            self.http
                .post(format!("{base_url}/exchange_github_app_token"))
        };
        let response = request.bearer_auth(token).send().await.map_err(|e| {
            AuthError::Transport(format!("App token exchange failed: {}", e.without_url()))
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let text = if root {
                let body: Value = serde_json::from_str(&text).map_err(|_| {
                    AuthError::Invalid("App token exchange response invalid JSON".into())
                })?;
                body.get("error")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_else(|| "undefined".into())
            } else {
                text
            };
            return Err(AuthError::Upstream {
                status: status.as_u16(),
                message: format!(
                    "App token exchange failed: {} {} - {text}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
            });
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| AuthError::Invalid(format!("App token exchange response invalid: {e}")))?;
        body.get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AuthError::Invalid("App token exchange response has no token".into()))
    }

    /// `revokeAppToken`: `DELETE /installation/token`. The baseline ignores the response, so
    /// this returns the status for logging but callers treat failure as non-fatal. Only
    /// tokens with `must_revoke` are sent.
    pub async fn revoke(&self, token: &AcquiredToken) -> Option<u16> {
        if !token.must_revoke() || token.token.is_empty() {
            return None;
        }
        let response = self
            .http
            .delete(format!("{}/installation/token", self.github_api))
            .bearer_auth(&token.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "opencode-ecosystem")
            .send()
            .await;
        match response {
            Ok(response) => Some(response.status().as_u16()),
            Err(_) => {
                tracing::warn!("revoking app token failed");
                None
            }
        }
    }
}
