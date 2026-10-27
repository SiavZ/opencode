//! Purpose: expose operator diagnostics over an authenticated JSON lookup endpoint.
//! Scope: read-only support HTTP handler.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! External:
//! |-- axum::{Json, extract::{Query, State}, http::{HeaderMap, StatusCode}}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: operator bearer is required before SQL; quota limits must be configured.
//! Contract tests: hosted/src/console/support/tests.rs

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use opencode_hosted_adapters::console::support::{SupportStore, authorized};
use serde_json::{Value, json};

use super::super::ConsoleState;
use super::actions::{Reply, failure};

pub(super) async fn lookup(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Reply {
    if !authorized(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        state.config.support.api_key.as_deref(),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Unauthorized"})),
        );
    }
    let Some(limits) = state
        .config
        .support
        .limits
        .as_ref()
        .filter(|limits| valid_limits(limits))
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Support quota limits are not configured"})),
        );
    };
    match SupportStore::new(state.db)
        .lookup(
            query
                .get("identifier")
                .map(|v| v.trim())
                .unwrap_or_default(),
            limits,
        )
        .await
    {
        Ok(result) => (StatusCode::OK, Json(result)),
        Err(error) => failure(error),
    }
}

fn valid_limits(limits: &Value) -> bool {
    [
        "rollingLimit",
        "rollingWindow",
        "weeklyLimit",
        "monthlyLimit",
    ]
    .iter()
    .all(|key| limits["lite"][key].as_i64().is_some())
        && ["20", "100", "200"].iter().all(|plan| {
            ["fixedLimit", "rollingLimit", "rollingWindow"]
                .iter()
                .all(|key| limits["black"][plan][key].as_i64().is_some())
        })
}
