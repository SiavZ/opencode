//! Purpose: router assembly: every protocol operation (current and legacy) mounted from
//! `opencode_protocol::registry()`, stream routes from p06.server-streams, `/doc`,
//! `/openapi.json`, UI fallback and the middleware stack (auth, CORS, compression).
//! Scope: process-level router construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/lib.rs
//! |-- protocol/src/openapi.rs
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/compression.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/middleware/errors.rs
//! `-- server/src/services/mod.rs
//! External:
//! |-- auth::layer
//! |-- axum::Router
//! |-- axum::extract::Path
//! |-- axum::extract::Request
//! |-- axum::extract::State
//! |-- axum::http::StatusCode
//! |-- axum::http::header
//! |-- axum::middleware::from_fn
//! |-- axum::middleware::from_fn_with_state
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- axum::routing::MethodFilter
//! |-- axum::routing::MethodRouter
//! |-- axum::routing::get
//! |-- axum::routing::on
//! |-- compression::layer
//! |-- cors::layer
//! |-- dispatch::path_params
//! |-- dispatch::run
//! |-- futures_util::FutureExt
//! |-- futures_util::future::BoxFuture
//! |-- header::CONTENT_SECURITY_POLICY
//! |-- header::CONTENT_TYPE
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- streams::owns
//! `-- streams::routes
//! Used by:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/handlers/streams/pty.rs
//! |-- server/src/lib.rs
//! |-- server/tests/http_auth.rs
//! |-- server/tests/http_cors.rs
//! |-- server/tests/http_errors.rs
//! |-- server/tests/http_legacy.rs
//! |-- server/tests/http_proxy.rs
//! |-- server/tests/replay_baseline.rs
//! |-- server/tests/sse_events.rs
//! |-- server/tests/support/mod.rs
//! `-- server/tests/ws_pty.rs
//! Invariants: path templates `:p` become `{p}` and a trailing `*` becomes `{*wildcard}` (exposed to
//! handlers as param `*`). Same-path operations share one method router. Layer order, outermost
//! first: CORS, compression, auth, routes (OPTIONS is answered by CORS before auth, like the
//! baseline). The embedded router mounts only the current API, no CORS/compression/UI, and
//! serves `/openapi.json` (packages/server/src/routes.ts). The full host serves `/doc`.
//! Contract tests: server/tests/http_routes.rs, server/tests/http_auth.rs, server/tests/http_cors.rs

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use opencode_protocol::{Api, Endpoint, Method};

use crate::handlers::{dispatch, streams};
use crate::middleware::auth::{self, AuthConfig};
use crate::middleware::compression;
use crate::middleware::cors::{self, CorsConfig};
use crate::middleware::errors::{HttpError, json};
use crate::services::Services;

/// Name used for a trailing `*` wildcard segment in axum paths.
pub const WILDCARD_PARAM: &str = "wildcard";

/// Shared per-router state, cloned per request.
#[derive(Clone)]
pub struct AppState {
    pub services: Services,
    /// Outbound client for the legacy remote-workspace proxy.
    pub client: reqwest::Client,
    pub config: Arc<ServerConfig>,
}

/// Host-level router options.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    pub auth: AuthConfig,
    /// Full host CORS policy. `None` disables the CORS layer (embedded host).
    pub cors: Option<CorsConfig>,
    /// Full host response compression (`middleware/compression.ts`).
    pub compression: bool,
    /// Mount the legacy PublicApi families, `/doc` and the UI fallback (full host).
    pub full_host: bool,
}

impl ServerConfig {
    /// Full host defaults (`HttpApiApp.createRoutes`): auth from the environment, CORS with no
    /// extra origins, compression on.
    pub fn full_host(auth: AuthConfig, cors: CorsConfig) -> Self {
        ServerConfig {
            auth,
            cors: Some(cors),
            compression: true,
            full_host: true,
        }
    }
}

/// Full host router (`HttpApiApp.createRoutes`).
pub fn router(services: Services, config: ServerConfig) -> Router {
    build(services, config)
}

/// Embedded router (`packages/server/src/routes.ts` `createEmbeddedRoutes`): current API only,
/// auth disabled, `/openapi.json`.
pub fn embedded_router(services: Services) -> Router {
    build(
        services,
        ServerConfig {
            auth: AuthConfig::default(),
            cors: None,
            compression: false,
            full_host: false,
        },
    )
}

/// `createRoutes(password)`: current API only with optional Basic auth.
pub fn current_router(services: Services, password: Option<String>) -> Router {
    build(
        services,
        ServerConfig {
            auth: AuthConfig {
                username: "opencode".to_string(),
                password,
            },
            cors: None,
            compression: false,
            full_host: false,
        },
    )
}

fn build(services: Services, config: ServerConfig) -> Router {
    let apis: Vec<&'static Api> = if config.full_host {
        opencode_protocol::registry().to_vec()
    } else {
        vec![opencode_protocol::current::api()]
    };
    router_with(&apis, services, config)
}

/// Router over an explicit API list (fixture replay, contract tests). `router`,
/// `embedded_router` and `current_router` are this with the registry APIs.
pub fn router_with(apis: &[&'static Api], services: Services, config: ServerConfig) -> Router {
    let config = Arc::new(config);
    let state = AppState {
        services,
        client: reqwest::Client::new(),
        config: config.clone(),
    };
    let mut paths: Vec<(String, MethodRouter<AppState>)> = Vec::new();
    for api in apis {
        for (_, endpoint) in api.endpoints() {
            if streams::owns(&endpoint.path) {
                continue;
            }
            let path = axum_path(&endpoint.path);
            let route = on(filter(endpoint.method), endpoint_handler(api, endpoint));
            match paths.iter_mut().find(|(p, _)| *p == path) {
                Some((_, existing)) => {
                    let merged = std::mem::take(existing).merge(route);
                    *existing = merged;
                }
                None => paths.push((path, route)),
            }
        }
    }
    let mut app: Router<AppState> = Router::new();
    for (path, route) in paths {
        app = app.route(&path, route);
    }
    app = app.merge(streams::routes());
    if config.full_host {
        app = app.route("/doc", axum::routing::get(doc)).fallback(ui);
    } else {
        app = app
            .route("/openapi.json", axum::routing::get(openapi))
            .fallback(|| async { StatusCode::NOT_FOUND });
    }
    let mut app = app
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(config.auth.clone()),
            auth::layer,
        ));
    if config.compression {
        app = app.layer(axum::middleware::from_fn(compression::layer));
    }
    if let Some(cors) = &config.cors {
        app = app.layer(axum::middleware::from_fn_with_state(
            Arc::new(cors.clone()),
            cors::layer,
        ));
    }
    app
}

type RawParams = Option<Path<Vec<(String, String)>>>;

fn endpoint_handler(
    api: &'static Api,
    endpoint: &'static Endpoint,
) -> impl Fn(State<AppState>, RawParams, Request) -> BoxFuture<'static, Response>
+ Clone
+ Send
+ Sync
+ 'static {
    move |State(state): State<AppState>, params, request: Request| {
        let raw = dispatch::path_params(params)
            .into_iter()
            .map(|(k, v)| {
                if k == WILDCARD_PARAM {
                    ("*".to_string(), v)
                } else {
                    (k, v)
                }
            })
            .collect();
        // Defects (panics) become the NamedError Unknown 500 instead of a dropped connection.
        std::panic::AssertUnwindSafe(dispatch::run(state, api, endpoint, raw, request))
            .catch_unwind()
            .map(|result| {
                result.unwrap_or_else(|_| {
                    HttpError::Defect("handler panicked".into()).into_response()
                })
            })
            .boxed()
    }
}

/// `:param` -> `{param}`, trailing `*` -> `{*wildcard}`.
pub fn axum_path(template: &str) -> String {
    template
        .split('/')
        .map(|segment| {
            if let Some(name) = segment.strip_prefix(':') {
                format!("{{{name}}}")
            } else if segment == "*" {
                format!("{{*{WILDCARD_PARAM}}}")
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn filter(method: Method) -> MethodFilter {
    match method {
        Method::Get => MethodFilter::GET,
        Method::Post => MethodFilter::POST,
        Method::Put => MethodFilter::PUT,
        Method::Patch => MethodFilter::PATCH,
        Method::Delete => MethodFilter::DELETE,
    }
}

async fn doc() -> Response {
    json(
        StatusCode::OK,
        &opencode_protocol::openapi::document(opencode_protocol::registry()),
    )
}

async fn openapi() -> Response {
    json(
        StatusCode::OK,
        &opencode_protocol::openapi::document(&[opencode_protocol::current::api()]),
    )
}

/// UI catch-all (`serveUIEffect`). Without a composed UI the request answers the typed 503.
async fn ui(State(state): State<AppState>, request: Request) -> Response {
    let Some(assets) = state.services.ui.as_ref() else {
        return HttpError::not_yet_ported("ui").into_response();
    };
    match assets.get(request.uri().path()).await {
        None => StatusCode::NOT_FOUND.into_response(),
        Some((content_type, body, csp)) => {
            let mut response = (StatusCode::OK, body).into_response();
            if let Ok(value) = content_type.parse() {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            if let Some(value) = csp.and_then(|v| v.parse().ok()) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_SECURITY_POLICY, value);
            }
            response
        }
    }
}
