//! Purpose: hosted native OpenAuth issuer HTTP endpoints and fail-closed configuration.
//! Scope: issuer origin routes separate from console external-issuer client routes.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::Router
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: missing settings disable issuance; partial settings fail startup; secrets redacted.
//! Contract tests: hosted-adapters issuer_runtime/tests.rs

use std::{collections::HashMap, sync::Arc};

use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use opencode_hosted_adapters::console::{
    auth::issuer_runtime::{
        BrowserResponse, Error, GithubConfig, GoogleConfig, IssuerConfig, IssuerRuntime,
        SqliteStore, TenantAdmission,
    },
    tenant::TenantStore,
};
use serde_json::json;

use super::state::ConsoleState;

#[cfg(test)]
mod tests;

pub fn router(state: ConsoleState) -> Router {
    routes().with_state(state)
}

#[derive(Clone, Default)]
pub struct Config {
    pub runtime: Option<Arc<IssuerRuntime>>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuerConfig")
            .field("configured", &self.runtime.is_some())
            .finish()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let names = [
            "OPENCODE_ISSUER_URL",
            "OPENCODE_ISSUER_STORE_PATH",
            "OPENCODE_ISSUER_SIGNING_KEY_FILE",
            "OPENCODE_ISSUER_KEY_ID",
            "OPENCODE_ISSUER_GITHUB_CLIENT_ID",
            "OPENCODE_ISSUER_GITHUB_CLIENT_SECRET",
            "OPENCODE_ISSUER_GOOGLE_CLIENT_ID",
        ];
        if names.iter().all(|name| std::env::var_os(name).is_none()) {
            return Ok(Self::default());
        }
        let required = |name: &str| -> anyhow::Result<String> {
            let value = std::env::var(name)
                .map_err(|_| anyhow::anyhow!("issuer setting {name} required"))?;
            anyhow::ensure!(!value.is_empty(), "issuer setting {name} required");
            Ok(value)
        };
        let store = SqliteStore::open(std::path::Path::new(&required(names[1])?))?;
        let retired_jwks = match std::env::var("OPENCODE_ISSUER_RETIRED_JWKS_FILE") {
            Ok(path) => serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)?["keys"]
                .as_array()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("retired JWKS must contain keys"))?,
            Err(_) => vec![],
        };
        let runtime = IssuerRuntime::new(
            IssuerConfig {
                issuer: required(names[0])?,
                signing_key_pem: std::fs::read(required(names[2])?)?,
                key_id: required(names[3])?,
                retired_jwks,
                github: GithubConfig {
                    client_id: required(names[4])?,
                    client_secret: required(names[5])?,
                    authorization_endpoint: "https://github.com/login/oauth/authorize".into(),
                    token_endpoint: "https://github.com/login/oauth/access_token".into(),
                    api_base: "https://api.github.com".into(),
                },
                google: GoogleConfig {
                    client_id: required(names[6])?,
                    issuer: "https://accounts.google.com".into(),
                },
            },
            Arc::new(store),
        )?;
        Ok(Self {
            runtime: Some(Arc::new(runtime)),
        })
    }
}

pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/authorize", get(authorize))
        .route("/token", post(token).options(preflight))
        .route(
            "/.well-known/oauth-authorization-server",
            get(discovery).options(preflight),
        )
        .route("/.well-known/jwks.json", get(jwks).options(preflight))
        .route("/userinfo", get(userinfo))
        .route("/{provider}/authorize", get(provider_authorize))
        .route("/github/callback", get(github_callback))
        .route("/google/callback", post(google_callback))
}

fn runtime(state: &ConsoleState) -> Result<&IssuerRuntime, Box<Response>> {
    state.config.issuer.runtime.as_deref().ok_or_else(|| {
        Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"issuer_unconfigured"})),
            )
                .into_response(),
        )
    })
}

fn failure(error: Error) -> Response {
    if let Error::OAuth {
        code,
        description,
        status,
    } = error
    {
        return finish(
            (
                StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
                Json(json!({"error":code,"error_description":description})),
            )
                .into_response(),
        );
    }
    let (status, code) = match error {
        Error::UnauthorizedClient => (StatusCode::BAD_REQUEST, "unauthorized_client"),
        Error::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_request"),
        Error::InvalidGrant | Error::Jwt(_) | Error::Provider => {
            (StatusCode::BAD_REQUEST, "invalid_grant")
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "server_error"),
    };
    finish((status, Json(json!({"error":code}))).into_response())
}

fn finish(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, "no-cache".parse().expect("static header"));
    response
}

fn browser(result: Result<BrowserResponse, Error>) -> Response {
    let response = match result {
        Ok(response) => response,
        Err(error) => return failure(error),
    };
    let mut output = if let Some(location) = response.location {
        match header::HeaderValue::from_str(&location) {
            Ok(location) => (StatusCode::FOUND, [(header::LOCATION, location)]).into_response(),
            Err(_) => return failure(Error::InvalidRequest),
        }
    } else {
        Html(response.html.unwrap_or_default()).into_response()
    };
    if let Some(cookie) = response.cookie
        && let Ok(cookie) = cookie.parse()
    {
        output.headers_mut().append(header::SET_COOKIE, cookie);
    }
    finish(output)
}

fn cookie<'a>(headers: &'a HeaderMap, runtime: &IssuerRuntime) -> &'a str {
    let mut matches = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| {
            part.trim()
                .strip_prefix(runtime.cookie_name())
                .and_then(|part| part.strip_prefix('='))
        });
    let first = matches.next().unwrap_or("");
    if matches.next().is_some() {
        return "";
    }
    first
}

async fn authorize(
    State(state): State<ConsoleState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    browser(runtime.authorize(&query).await)
}

async fn provider_authorize(
    State(state): State<ConsoleState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    browser(
        runtime
            .start_provider(&provider, cookie(&headers, runtime))
            .await,
    )
}

async fn github_callback(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Query(values): Query<HashMap<String, String>>,
) -> Response {
    callback(state, headers, values, "github").await
}

async fn google_callback(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Form(values): Form<HashMap<String, String>>,
) -> Response {
    callback(state, headers, values, "google").await
}

async fn callback(
    state: ConsoleState,
    headers: HeaderMap,
    values: HashMap<String, String>,
    provider: &str,
) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    let admission = TenantAdmission {
        tenant: TenantStore::new(state.db.clone()),
        stage: state.config.stage.clone(),
    };
    browser(
        runtime
            .callback(provider, &values, cookie(&headers, runtime), &admission)
            .await,
    )
}

async fn token(
    State(state): State<ConsoleState>,
    Form(values): Form<HashMap<String, String>>,
) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    cors(match runtime.token(&values) {
        Ok(tokens) => finish(Json(tokens).into_response()),
        Err(error) => failure(error),
    })
}

async fn discovery(State(state): State<ConsoleState>) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    cors(Json(runtime.discovery()).into_response())
}

async fn jwks(State(state): State<ConsoleState>) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    cors(Json(runtime.jwks()).into_response())
}

async fn userinfo(State(state): State<ConsoleState>, headers: HeaderMap) -> Response {
    let runtime = match runtime(&state) {
        Ok(runtime) => runtime,
        Err(response) => return *response,
    };
    let Some(bearer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return failure(Error::InvalidRequest);
    };
    match runtime.userinfo(bearer) {
        Ok(value) => finish(Json(value).into_response()),
        Err(error) => failure(error),
    }
}

fn cors(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        "*".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "*".parse().expect("static header"),
    );
    response
}

async fn preflight() -> Response {
    cors(StatusCode::NO_CONTENT.into_response())
}
