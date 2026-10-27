//! Purpose: console auth HTTP routes and reusable cookie/session/actor boundary.
//! Scope: request handling over the existing OpenAuth, iron and tenant adapters.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth/oauth_tests.rs
//! |-- hosted/src/console/auth/request.rs
//! |-- hosted/src/console/auth/session.rs
//! |-- hosted/src/console/auth/tests.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- axum::Router
//! |-- reqwest::Client
//! `-- url::Url
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/workspace.rs
//! Invariants: existing auth paths follow the TS routes; secrets never appear in Debug.
//! Contract tests: hosted/src/console/auth/{tests,oauth_tests}.rs, hosted/src/console/tests.rs

#![allow(clippy::result_large_err)]

#[cfg(test)]
mod oauth_tests;
mod request;
mod session;
#[cfg(test)]
mod tests;

pub use request::require_same_origin;
pub use session::{read_session, resolve_actor, with_session};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use opencode_hosted_adapters::console::auth::AuthClient;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::identity::{callback_next_path, new_console_redirect};
use serde::Deserialize;
use serde_json::json;

use super::ConsoleState;

/// Environment configuration corresponding to VITE_AUTH_URL, ZEN_SESSION_SECRET and migration URL.
#[derive(Clone, Default)]
pub struct Config {
    pub issuer_url: Option<String>,
    pub session_secret: Option<String>,
    pub state_redis_url: Option<String>,
    pub console_url: Option<String>,
    /// Trusted external origin used by the reverse-proxied console.
    pub public_origin: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("issuer_configured", &self.issuer_url.is_some())
            .field("session_configured", &self.session_secret.is_some())
            .field("state_store_configured", &self.state_redis_url.is_some())
            .field("console_configured", &self.console_url.is_some())
            .field("public_origin_configured", &self.public_origin.is_some())
            .finish()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let value = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|s| !s.is_empty()))
        };
        let config = Self {
            issuer_url: value(&["OPENCODE_CONSOLE_AUTH_URL", "VITE_AUTH_URL"]),
            session_secret: value(&["OPENCODE_CONSOLE_SESSION_SECRET", "ZEN_SESSION_SECRET"]),
            state_redis_url: value(&["OPENCODE_CONSOLE_AUTH_STATE_REDIS_URL"]),
            console_url: value(&["OPENCODE_CONSOLE_MIGRATION_URL", "CONSOLE_MIGRATION_URL"]),
            public_origin: value(&["OPENCODE_CONSOLE_PUBLIC_ORIGIN"]),
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if let Some(issuer) = &self.issuer_url {
            let url = url::Url::parse(issuer)
                .map_err(|_| anyhow::anyhow!("Invalid console auth issuer URL"))?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                "Invalid console auth issuer URL"
            );
        }
        if let Some(origin) = &self.public_origin {
            request::parse_origin(origin)
                .map_err(|_| anyhow::anyhow!("Invalid console public origin"))?;
        } else {
            anyhow::bail!("OPENCODE_CONSOLE_PUBLIC_ORIGIN is required");
        }
        anyhow::ensure!(
            self.state_redis_url
                .as_deref()
                .is_some_and(|url| !url.is_empty()),
            "OPENCODE_CONSOLE_AUTH_STATE_REDIS_URL is required"
        );
        Ok(())
    }

    fn client(&self) -> Result<AuthClient, Response> {
        let issuer = self
            .issuer_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Console auth issuer URL is not configured",
                )
            })?;
        let state_store = self
            .state_redis_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Console auth state store is not configured",
                )
            })?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not initialize auth client",
                )
            })?;
        AuthClient::new(issuer, http)
            .with_state_store(state_store)
            .map_err(|_| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Console auth state store is unavailable",
                )
            })
    }
}

pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/auth", get(index))
        .route("/auth/authorize", get(authorize))
        .route("/auth/callback", get(callback))
        .route("/auth/callback/{*path}", get(callback))
        .route("/auth/status", get(status))
        .route("/auth/logout", get(logout))
        // New JSON action transport. This is not Solid server-function wire compatibility.
        .route("/auth/action/actor", post(actor_action))
}

#[derive(Default, Deserialize)]
struct AuthorizeQuery {
    #[serde(rename = "continue")]
    continuation: Option<String>,
}

async fn authorize(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, Response> {
    let request = request::url(&state.config.auth, &uri, &headers)?;
    if let Some(next) = query.continuation.as_deref() {
        request::require_local_path(next)?;
    }
    let callback = request
        .join(&format!(
            "./callback{}",
            query.continuation.as_deref().unwrap_or("")
        ))
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid callback URL"))?;
    let session = read_session(&state, &headers)?;
    let client = state.config.auth.client()?;
    let oauth_state = client.create_oauth_state(&session.id).await.map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Could not create authorization state",
        )
    })?;
    let response = redirect(
        &client.authorize_url(callback.as_str(), &oauth_state),
        302,
        false,
    )?;
    with_session(&state, &session, response)
}

async fn callback(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let request = request::url(&state.config.auth, &uri, &headers)?;
    let cause: serde_json::Map<String, serde_json::Value> = request
        .query_pairs()
        .filter(|(key, _)| key != "code" && key != "state")
        .map(|(k, v)| (k.into_owned(), json!(v.into_owned())))
        .collect();
    let failure = |message: String| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": message, "cause": cause})),
        )
            .into_response()
    };
    let code = request
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .filter(|code| !code.is_empty())
        .ok_or_else(|| failure(request::missing_code_message(&headers).into()))?;
    let returned_state = request
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| failure("Invalid authorization state".into()))?;
    let session = read_session(&state, &headers)?;
    let client = state.config.auth.client()?;
    client
        .consume_oauth_state(&returned_state, &session.id)
        .await
        .map_err(|_| failure("Invalid authorization state".into()))?;
    let mut callback = request.clone();
    callback.set_query(None);
    callback.set_fragment(None);
    let token = client
        .exchange(&code, callback.as_str())
        .await
        .map_err(|_| failure("Authorization code exchange failed".into()))?;
    let subject = client
        .verify_account_subject(&token)
        .await
        .map_err(|_| failure("Invalid access token".into()))?;
    let mut session = session;
    session.data.login(&subject.account_id, &subject.email);
    let next = callback_next_path(request.path());
    request::require_local_path(&next)?;
    let next = request::localized_path(&headers, &next);
    with_session(&state, &session, redirect(&next, 302, false)?)
}

async fn status(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let session = read_session(&state, &headers)?;
    with_session(&state, &session, Json(&session.data).into_response())
}

async fn logout(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let mut session = read_session(&state, &headers)?;
    session.data.logout();
    with_session(&state, &session, redirect("/zen", 302, false)?)
}

async fn index(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let mut session = match read_session(&state, &headers) {
        Ok(session) => session,
        Err(_) => return redirect("/auth/authorize", 302, false),
    };
    let actor = match resolve_actor(&state, &mut session, None, &Method::GET).await {
        Ok(actor) => actor,
        Err(response) if response.status().is_redirection() => return Err(response),
        Err(_) => return redirect("/auth/authorize", 302, false),
    };
    let Ok(account) = actor.account() else {
        return redirect("/auth/authorize", 302, false);
    };
    let store = TenantStore::new(state.db.clone());
    let workspace = match store.last_seen_workspace(account).await {
        Ok(value) => value,
        Err(_) => return redirect("/auth/authorize", 302, false),
    };
    let response = match workspace {
        Some(workspace) => redirect(&format!("/workspace/{workspace}"), 302, false)?,
        None => {
            let next = match new_console_redirect(state.config.auth.console_url.as_deref()) {
                Ok(next) => next,
                Err(_) => return redirect("/auth/authorize", 302, false),
            };
            redirect(&next.location, next.status, next.no_store)?
        }
    };
    with_session(&state, &session, response)
}

#[derive(Deserialize)]
struct ActorInput {
    #[serde(rename = "workspaceID")]
    workspace_id: Option<String>,
}

/// Explicit Rust JSON transport for reusable actor resolution, not a baseline REST endpoint.
async fn actor_action(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Json(input): Json<ActorInput>,
) -> Result<Response, Response> {
    require_same_origin(&state, &headers)?;
    let mut session = read_session(&state, &headers)?;
    let actor = resolve_actor(
        &state,
        &mut session,
        input.workspace_id.as_deref(),
        &Method::POST,
    )
    .await?;
    with_session(&state, &session, Json(actor).into_response())
}

pub(super) fn redirect(location: &str, status: u16, no_store: bool) -> Result<Response, Response> {
    let mut response = StatusCode::from_u16(status)
        .unwrap_or(StatusCode::FOUND)
        .into_response();
    response.headers_mut().insert(
        header::LOCATION,
        location.parse().map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid redirect destination",
            )
        })?,
    );
    if no_store {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
    }
    Ok(response)
}

pub(super) fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}
