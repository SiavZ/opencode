//! Purpose: GitHub App and repository REST calls used by the token exchange: App JWT signing
//! (`universal-github-app-jwt`: iat = now-30, exp = iat+600, iss = app id, RS256, PKCS#8 PEM
//! with escaped `\n` accepted), `GET /repos/{owner}/{repo}/installation`,
//! `POST /app/installations/{id}/access_tokens` (no body, matching `createAppAuth` without
//! permissions) and `GET /repos/{owner}/{repo}` for the PAT write-permission check
//! (`packages/function/src/api.ts:286-358`). Error messages follow Octokit `RequestError`.
//! Scope: per-process client, base URL injectable for fixture servers.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/github/auth/mod.rs
//! External:
//! |-- jsonwebtoken::{encode, EncodingKey, Header}
//! |-- reqwest::Client
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/github/auth/handlers.rs
//! |-- ecosystem/src/github/auth/mod.rs
//! `-- ecosystem/tests/github_auth_exchange.rs
//! Invariants: the private key never appears in Debug output or errors, installation tokens
//! are only returned to the caller, never logged.
//! Contract tests: ecosystem/tests/github_auth_exchange.rs

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::Value;

use super::AuthError;

pub const GITHUB_API: &str = "https://api.github.com";
const USER_AGENT: &str = "opencode-ecosystem";

/// GitHub App credentials (`GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`).
#[derive(Clone)]
pub struct AppCredentials {
    pub app_id: String,
    key: EncodingKey,
}

impl std::fmt::Debug for AppCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCredentials")
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

impl AppCredentials {
    pub fn from_pem(app_id: impl Into<String>, private_key: &str) -> Result<Self, AuthError> {
        let pem = private_key.replace("\\n", "\n");
        let key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| AuthError::Config(format!("invalid GitHub App private key: {e}")))?;
        Ok(Self {
            app_id: app_id.into(),
            key,
        })
    }

    /// Sign the App JWT for `now` (unix seconds).
    pub fn app_jwt(&self, now: u64) -> Result<String, AuthError> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iat: u64,
            exp: u64,
            iss: &'a str,
        }
        let iat = now.saturating_sub(30);
        let claims = Claims {
            iat,
            exp: iat + 600,
            iss: &self.app_id,
        };
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| AuthError::Config(format!("failed to sign GitHub App JWT: {e}")))
    }
}

/// Repository permissions from `GET /repos/{owner}/{repo}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoPermissions {
    pub admin: bool,
    pub push: bool,
    pub maintain: bool,
}

/// Minimal GitHub REST client for the exchange flows.
#[derive(Debug, Clone)]
pub struct GitHubApi {
    base: String,
    http: reqwest::Client,
}

impl Default for GitHubApi {
    fn default() -> Self {
        Self::new(GITHUB_API)
    }
}

impl GitHubApi {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// `GET /repos/{owner}/{repo}/installation` with the App JWT. Returns the raw
    /// installation object so the lookup route can echo it unchanged.
    pub async fn repo_installation(
        &self,
        app_jwt: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Value, AuthError> {
        let url = format!(
            "{}/repos/{}/{}/installation",
            self.base,
            enc(owner),
            enc(repo)
        );
        self.send(self.http.get(url).bearer_auth(app_jwt)).await
    }

    /// `POST /app/installations/{id}/access_tokens` and return the token string.
    pub async fn installation_token(
        &self,
        app_jwt: &str,
        installation_id: u64,
    ) -> Result<String, AuthError> {
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.base
        );
        let body = self.send(self.http.post(url).bearer_auth(app_jwt)).await?;
        body.get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AuthError::Invalid("access token response has no token".into()))
    }

    /// Full App flow used by both exchange routes: JWT, installation lookup, token.
    pub async fn app_installation_token(
        &self,
        app: &AppCredentials,
        now: u64,
        owner: &str,
        repo: &str,
    ) -> Result<String, AuthError> {
        let jwt = app.app_jwt(now)?;
        let installation = self.repo_installation(&jwt, owner, repo).await?;
        let id = installation
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| AuthError::Invalid("installation response has no id".into()))?;
        self.installation_token(&app.app_jwt(now)?, id).await
    }

    /// `GET /repos/{owner}/{repo}` with the user's token and read `permissions`.
    pub async fn repo_permissions(
        &self,
        user_token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<RepoPermissions, AuthError> {
        let url = format!("{}/repos/{}/{}", self.base, enc(owner), enc(repo));
        let body = self
            .send(
                self.http
                    .get(url)
                    .header("authorization", format!("token {user_token}")),
            )
            .await?;
        let flag = |name: &str| {
            body.pointer(&format!("/permissions/{name}"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        // Baseline reads `repoData.permissions.admin` and throws a TypeError when the
        // permissions object is absent (unauthenticated view). Treat absence as no access.
        if body.get("permissions").is_none() {
            return Err(AuthError::PermissionDenied(
                "Cannot read properties of undefined (reading 'admin')".into(),
            ));
        }
        Ok(RepoPermissions {
            admin: flag("admin"),
            push: flag("push"),
            maintain: flag("maintain"),
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, AuthError> {
        let response = request
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", USER_AGENT)
            .send()
            .await
            .map_err(|e| AuthError::Transport(e.without_url().to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| AuthError::Transport(e.without_url().to_string()))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        if (200..300).contains(&status) {
            return Ok(body);
        }
        Err(AuthError::Upstream {
            status,
            message: octokit_message(&body),
        })
    }
}

/// Port of `@octokit/request` `toErrorMessage`.
pub fn octokit_message(data: &Value) -> String {
    match data {
        Value::String(s) => s.clone(),
        Value::Object(map) if map.contains_key("message") => {
            let message = match &map["message"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let suffix = match map.get("documentation_url") {
                Some(Value::String(url)) => format!(" - {url}"),
                Some(other) => format!(" - {other}"),
                None => String::new(),
            };
            match map.get("errors") {
                Some(Value::Array(errors)) => {
                    let joined: Vec<String> = errors.iter().map(Value::to_string).collect();
                    format!("{message}: {}{suffix}", joined.join(", "))
                }
                _ => format!("{message}{suffix}"),
            }
        }
        other => format!("Unknown error: {other}"),
    }
}

fn enc(segment: &str) -> String {
    url::form_urlencoded::byte_serialize(segment.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
