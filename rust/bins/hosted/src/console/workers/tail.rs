//! Purpose: accept authenticated native trace-tail batches and deliver enriched records to configured capture.
//! Scope: separate native log processor HTTP process.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/support.rs
//! |-- hosted-adapters/src/console/workers.rs
//! `-- hosted-adapters/src/console/workers/logs.rs
//! External:
//! |-- axum::{Router,Json}
//! |-- serde_json::{Value,json}
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: authorization precedes body parsing; ingress never acknowledges before capture completes.
//! Contract tests: hosted/src/console/workers/tail_tests.rs

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use opencode_hosted_adapters::console::{
    support::authorized,
    workers::{Capture, logs},
};
use serde_json::{Value, json};
use time::OffsetDateTime;

#[derive(Clone)]
struct TailState {
    token: String,
    capture: Capture,
    max_body: usize,
}

pub fn router(token: String, capture: Capture, max_body: usize) -> Router {
    Router::new()
        .route("/tail", post(ingest))
        .with_state(TailState {
            token,
            capture,
            max_body,
        })
}

async fn ingest(State(state): State<TailState>, request: Request<Body>) -> Response {
    if !authorized(
        request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some(&state.token),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Unauthorized"})),
        )
            .into_response();
    }
    let body = match axum::body::to_bytes(request.into_body(), state.max_body).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error":"Tail batch exceeds limit"})),
            )
                .into_response();
        }
    };
    let traces = match serde_json::from_slice::<Vec<Value>>(&body) {
        Ok(traces) => traces,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Expected trace array"})),
            )
                .into_response();
        }
    };
    let result = match logs::enrich(&traces, OffsetDateTime::now_utc()) {
        Ok(result) => result,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Invalid trace batch"})),
            )
                .into_response();
        }
    };
    if state.capture.send(&result).await.is_err() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":"Tail capture failed"})),
        )
            .into_response();
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"accepted":result["console"].as_array().map_or(0,Vec::len)})),
    )
        .into_response()
}

pub async fn serve() -> anyhow::Result<()> {
    let token = std::env::var("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN")?;
    anyhow::ensure!(
        !token.is_empty(),
        "OPENCODE_CONSOLE_LOG_INGRESS_TOKEN must not be empty"
    );
    let capture = super::capture_from_env()?.ok_or_else(|| {
        anyhow::anyhow!("log ingress requires an explicit worker capture destination")
    })?;
    let addr = std::env::var("OPENCODE_HOSTED_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        super::tail_router(token, capture, 8 * 1024 * 1024),
    )
    .await?;
    Ok(())
}
