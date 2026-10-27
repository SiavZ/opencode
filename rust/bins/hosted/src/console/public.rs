//! Purpose: serve console metadata, fixed redirects, benchmark submission and upstream proxies.
//! Scope: auxiliary public HTTP routes; rendering remains with the compiled frontend.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/public/locale.rs
//! |-- hosted/src/console/public/proxy.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/public.rs
//! External:
//! |-- axum::{Json, Router}
//! |-- reqwest::Client
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: upstream destinations are configured once, never selected by request headers.
//! Contract tests: public/tests.rs

use super::ConsoleState;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

mod changelog;
mod data;
#[cfg(test)]
mod data_tests;
#[cfg(test)]
mod failure_tests;
mod github;
pub mod locale;
mod proxy;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct Config {
    pub openapi_url: String,
    pub update_origin: String,
    pub docs_origin: Option<String>,
    pub enterprise_origin: String,
    pub data_origin: Option<String>,
    pub releases_url: String,
    pub github_api_base: String,
    pub http: reqwest::Client,
    pub manual_http: reqwest::Client,
    pub cache: data::Cache,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            openapi_url: "https://raw.githubusercontent.com/anomalyco/opencode/refs/heads/dev/packages/sdk/openapi.json".into(),
            update_origin: "https://opencode.ai".into(),
            docs_origin: None,
            enterprise_origin: "https://enterprise.opencode.ai".into(),
            data_origin: None,
            releases_url: "https://api.github.com/repos/anomalyco/opencode/releases?per_page=20".into(),
            github_api_base: "https://api.github.com/repos/anomalyco/opencode".into(),
            cache: data::Cache::default(),
            manual_http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("HTTP client"),
            http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::limited(20)).build().expect("HTTP client"),
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::default())
    }
}

pub fn routes() -> Router<ConsoleState> {
    let routes = Router::new()
        .route("/oauth/opencode/client.json", get(metadata))
        .route("/openapi.json", get(openapi))
        .route("/github.json", get(github::get))
        .route("/discord", get(|| async { redirect("https://discord.gg/opencode", StatusCode::FOUND) }))
        .route("/feishu", get(|| async { redirect("https://applink.feishu.cn/client/chat/chatter/add_by_link?link_token=52ao9352-5623-4fa0-b7dd-3407c392c1af&qr_code=true", StatusCode::FOUND) }))
        .route("/desktop-feedback", get(|| async { redirect("https://discord.gg/h5TNnkFVNy", StatusCode::FOUND) }))
        .route("/stats", methods(proxy::stats))
        .route("/stats/", methods(proxy::stats))
        .route("/stats/{*path}", methods(proxy::stats))
        .route("/docs", methods(proxy::forward))
        .route("/docs/", methods(proxy::forward))
        .route("/docs/{*path}", methods(proxy::forward))
        .route("/s/{id}", methods(proxy::forward))
        .route("/t/{*path}", methods(proxy::forward))
        .route("/download/{channel}/{platform}", get(proxy::download))
        .route("/bench/submission", post(submission))
        .route("/benchmarks", get(benchmarks))
        .route("/benchmarks/{id}/tasks/{task}", get(benchmark_task))
        .route("/data", methods(data::proxy))
        .route("/data/", methods(data::proxy))
        .route("/data/{*path}", methods(data::proxy))
        .route("/changelog.json", get(changelog::get).options(changelog::options));
    locale::LOCALES.into_iter().fold(routes.clone(), |router, selected| {
        router.nest(&format!("/{selected}"), routes.clone().layer(axum::middleware::from_fn(move |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
            request.headers_mut().insert("x-opencode-locale", selected.parse().unwrap());
            let mut response = next.run(request).await;
            response.headers_mut().append(axum::http::header::SET_COOKIE, locale::cookie(selected).parse().unwrap());
            response
        })))
    })
}

fn methods<H, T>(handler: H) -> axum::routing::MethodRouter<ConsoleState>
where
    H: axum::handler::Handler<T, ConsoleState>,
    T: 'static,
{
    axum::routing::on(
        axum::routing::MethodFilter::GET
            .or(axum::routing::MethodFilter::POST)
            .or(axum::routing::MethodFilter::PUT)
            .or(axum::routing::MethodFilter::DELETE)
            .or(axum::routing::MethodFilter::OPTIONS)
            .or(axum::routing::MethodFilter::PATCH),
        handler,
    )
}

pub struct Error(Box<Response>);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        *self.0
    }
}

fn failure(status: StatusCode, message: &str) -> Error {
    Error(Box::new(
        (status, Json(json!({"error": message}))).into_response(),
    ))
}

fn redirect(location: &str, status: StatusCode) -> Response {
    (status, [("location", location)]).into_response()
}

fn origin(state: &ConsoleState) -> Result<url::Url, Error> {
    state
        .config
        .auth
        .public_origin
        .as_deref()
        .and_then(|value| url::Url::parse(value).ok())
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| {
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Console public origin is not configured",
            )
        })
}

async fn metadata(State(state): State<ConsoleState>) -> Result<Response, Error> {
    let origin = origin(&state)?.origin().ascii_serialization();
    let mut response = Json(json!({
        "client_id": format!("{origin}/oauth/opencode/client.json"), "client_name": "opencode",
        "client_uri": origin, "logo_uri": format!("{origin}/web-app-manifest-512x512.png"),
        "application_type": "native", "redirect_uris": ["http://127.0.0.1/callback", "http://localhost/callback"],
        "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
        "token_endpoint_auth_method": "none", "token_endpoint_auth_methods_supported": ["none"]
    })).into_response();
    response
        .headers_mut()
        .insert("cache-control", "public, max-age=300".parse().unwrap());
    response
        .headers_mut()
        .insert("access-control-allow-origin", "*".parse().unwrap());
    Ok(response)
}

async fn openapi(State(state): State<ConsoleState>) -> Result<Json<Value>, Error> {
    let response = state
        .config
        .public
        .http
        .get(&state.config.public.openapi_url)
        .send()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "OpenAPI upstream request failed"))?;
    response
        .json()
        .await
        .map(Json)
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Invalid OpenAPI upstream JSON"))
}

async fn benchmarks(State(state): State<ConsoleState>) -> Result<Json<Value>, Error> {
    opencode_hosted_adapters::console::public::benchmarks(&state.db)
        .await
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "benchmark list failed");
            failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })
}

async fn benchmark_task(
    State(state): State<ConsoleState>,
    axum::extract::Path((id, task)): axum::extract::Path<(String, String)>,
) -> Result<Json<Value>, Error> {
    opencode_hosted_adapters::console::public::benchmark_task(&state.db, &id, &task)
        .await
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "benchmark task failed");
            failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })
}

async fn submission(
    State(state): State<ConsoleState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Error> {
    let fields = ["model", "agent", "result"].map(|key| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    });
    let [Some(model), Some(agent), Some(result)] = fields else {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            locale::required(locale::select(&headers, uri.path())),
        ));
    };
    opencode_hosted_adapters::console::public::submit_benchmark(&state.db, model, agent, result)
        .await
        .map_err(|error| {
            tracing::error!(%error, "benchmark submission failed");
            failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })?;
    Ok(Json(json!({"success": true})))
}
