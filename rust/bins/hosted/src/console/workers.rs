//! Purpose: expose native stat HTTP worker and stdin console worker commands.
//! Scope: hosted worker process entrypoints, separate from authenticated console HTTP routes.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/logs.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-adapters/src/console/workers/stat.rs
//! External:
//! |-- anyhow::Context
//! |-- axum::{Router, Json}
//! |-- serde_json::Value
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/main.rs
//! Invariants: operator commands never become public HTTP routes; mutation requires disposable loopback DB confirmation.
//! Contract tests: hosted/src/console/workers/tests.rs

pub mod producer;
mod tail;
#[cfg(test)]
mod tail_tests;
#[cfg(test)]
mod tests;

pub use tail::router as tail_router;

pub async fn serve_logs() -> anyhow::Result<()> {
    tail::serve().await
}

use anyhow::Context;
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use opencode_hosted_adapters::console::{
    db::ConsoleDb,
    workers::{
        Capture, logs, operators,
        stat::{StatRequest, StatStore},
    },
};
use serde_json::Value;
use std::io::Read;
use time::OffsetDateTime;

pub fn stat_router(db: ConsoleDb) -> Router {
    Router::new().fallback(dispatch_stat).with_state(db)
}

async fn dispatch_stat(State(db): State<ConsoleDb>, method: Method, body: Bytes) -> Response {
    if method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response();
    }
    stat(State(db), body).await.into_response()
}

async fn stat(
    State(db): State<ConsoleDb>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    let request: StatRequest = serde_json::from_slice(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request".into()))?;
    StatStore::new(db)
        .aggregate(&request.ids, OffsetDateTime::now_utc())
        .await
        .map(Json)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stat aggregation failed".into(),
            )
        })
}

pub async fn serve_stat(db: ConsoleDb) -> anyhow::Result<()> {
    let addr = std::env::var("OPENCODE_HOSTED_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, stat_router(db)).await?;
    Ok(())
}

fn capture_from_env() -> anyhow::Result<Option<Capture>> {
    let file = std::env::var_os("OPENCODE_CONSOLE_WORKER_CAPTURE_FILE");
    let endpoint = std::env::var("OPENCODE_CONSOLE_WORKER_CAPTURE_URL").ok();
    anyhow::ensure!(
        file.is_none() || endpoint.is_none(),
        "configure only one worker capture destination"
    );
    if let Some(file) = file {
        return Ok(Some(Capture::JsonLines(file.into())));
    }
    if let Some(endpoint) = endpoint {
        let endpoint = url::Url::parse(&endpoint)?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https") && endpoint.host_str().is_some(),
            "capture URL must be HTTP(S)"
        );
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        return Ok(Some(Capture::Http {
            client,
            endpoint,
            bearer: std::env::var("OPENCODE_CONSOLE_WORKER_CAPTURE_TOKEN").ok(),
        }));
    }
    Ok(None)
}

pub async fn run(command: &str, input: Value, db: &ConsoleDb) -> anyhow::Result<Value> {
    if command == "inventory" {
        return Ok(operators::inventory());
    }
    if operators::config::handles(command) {
        return Ok(operators::config::run(command, &input).await?);
    }
    if command == "stat" {
        let request: StatRequest = serde_json::from_value(input)?;
        return Ok(StatStore::new(db.clone())
            .aggregate(&request.ids, OffsetDateTime::now_utc())
            .await?);
    }
    if command == "log-processor" {
        let traces = input
            .as_array()
            .context("log-processor expects an array of tail traces")?;
        return Ok(logs::enrich(traces, OffsetDateTime::now_utc())?);
    }
    let url = std::env::var("OPENCODE_CONSOLE_DATABASE_URL")
        .context("OPENCODE_CONSOLE_DATABASE_URL is required")?;
    let confirmation =
        std::env::var("OPENCODE_CONSOLE_OPERATOR_CONFIRM_DATABASE").unwrap_or_default();
    operators::validate_local(&url, db.database(), &confirmation)?;
    let mut input = input;
    input
        .as_object_mut()
        .context("operator input must be an object")?
        .insert("confirmDatabase".into(), Value::String(confirmation));
    Ok(operators::run(command, &input, db).await?)
}

/// Reads one bounded JSON document and prints one result. No implicit Honeycomb upload.
pub async fn run_from_env(db: ConsoleDb) -> anyhow::Result<()> {
    let command = std::env::var("OPENCODE_CONSOLE_WORKER_COMMAND")
        .context("OPENCODE_CONSOLE_WORKER_COMMAND is required")?;
    let input = tokio::task::spawn_blocking(|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "worker input exceeds 8 MiB");
        Ok::<_, anyhow::Error>(serde_json::from_slice::<Value>(&bytes)?)
    })
    .await??;
    let capture = capture_from_env()?;
    let result = run(&command, input, &db).await?;
    if let Some(capture) = capture {
        capture.send(&result).await?;
    }
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
