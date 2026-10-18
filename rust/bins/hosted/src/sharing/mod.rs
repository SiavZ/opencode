//! Purpose: sharing HTTP composition for the hosted binary: enterprise API router with hono
//! `cors()` default behavior, plus the public share API (`/share_*`) router.
//! Scope: router assembly and the CORS middleware.
//! Uses:
//! rust/bins/
//! |-- hosted/src/sharing/enterprise.rs
//! |-- hosted/src/sharing/public.rs
//! |-- hosted/src/sharing/tests.rs
//! `-- hosted/src/sharing/validate.rs
//! External:
//! `-- axum::{middleware, Router}
//! Used by:
//! rust/bins/
//! `-- hosted/src/main.rs
//! Invariants: hono cors defaults: `access-control-allow-origin: *` on every response, preflight
//! (OPTIONS) answers 204 with allow-methods `GET,HEAD,PUT,POST,DELETE,PATCH`, echoes requested
//! headers and sets `vary: Access-Control-Request-Headers` (captured in http.json).
//! Contract tests: bins/hosted/src/sharing/tests.rs

pub mod enterprise;
pub mod public;
#[cfg(test)]
mod tests;
pub mod validate;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;

/// hono `cors()` with default options.
pub async fn cors(request: Request, next: Next) -> Response {
    if request.method() == Method::OPTIONS {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        let headers = response.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,HEAD,PUT,POST,DELETE,PATCH"),
        );
        if let Some(requested) = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        {
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, requested.clone());
            headers.insert(
                header::VARY,
                HeaderValue::from_static("Access-Control-Request-Headers"),
            );
        }
        return response;
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

/// Enterprise share API (`packages/enterprise` Worker routes).
pub fn enterprise_router(state: enterprise::EnterpriseState) -> Router {
    enterprise::routes()
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
                "404 Not Found",
            )
        })
        .layer(middleware::from_fn(cors))
        .with_state(state)
}

/// Public share API (`packages/function` Worker routes), no CORS middleware in the baseline.
pub fn public_router(state: public::PublicState) -> Router {
    public::routes().with_state(state)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

async fn listen(router: Router) -> anyhow::Result<()> {
    let addr = env("OPENCODE_HOSTED_ADDR").unwrap_or_else(|| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// `OPENCODE_HOSTED_SERVICE=enterprise-share`: `OPENCODE_STORAGE_*` select S3 or R2
/// (infra/enterprise.ts:11-17), `SUPPORT_API_KEY` guards the support route.
pub async fn serve_enterprise() -> anyhow::Result<()> {
    let store = opencode_hosted_adapters::sharing::objects::S3Store::from_settings(env)?;
    let state = enterprise::EnterpriseState {
        store: std::sync::Arc::new(store),
        node_env_test: env("NODE_ENV").as_deref() == Some("test"),
        support_api_key: env("SUPPORT_API_KEY").unwrap_or_default(),
    };
    listen(enterprise_router(state)).await
}

/// `OPENCODE_HOSTED_SERVICE=public-share`: the share bucket uses the same `OPENCODE_STORAGE_*`
/// settings (the Worker binds `Bucket`, infra/app.ts:11), `WEB_DOMAIN` and `ADMIN_SECRET`.
/// Durable Object state is process-local here (status.md blocker H13-public-do).
pub async fn serve_public() -> anyhow::Result<()> {
    let bucket = opencode_hosted_adapters::sharing::objects::S3Store::from_settings(env)?;
    let state = public::PublicState {
        namespace: opencode_hosted_adapters::sharing::public::SyncNamespace::new(
            std::sync::Arc::new(bucket),
        ),
        web_domain: env("WEB_DOMAIN").unwrap_or_default(),
        admin_secret: env("ADMIN_SECRET").unwrap_or_default(),
    };
    listen(public_router(state)).await
}
