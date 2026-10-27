//! Purpose: validate enterprise inquiries, swallow bot submissions and apply delivery success rules.
//! Scope: public enterprise HTTP handler.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/support/enterprise.rs
//! External:
//! |-- axum::{Json, body::Bytes, extract::State}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: honeypot returns success without delivery; production requires one successful channel.
//! Contract tests: hosted/src/console/support/tests.rs

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use opencode_hosted_adapters::console::support::enterprise::{EnterpriseClient, Inquiry};
use serde_json::{Value, json};

use super::super::ConsoleState;
use super::actions::{Reply, valid_email};

pub(super) async fn email_signup(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let email = if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"))
    {
        url::form_urlencoded::parse(&body)
            .find(|(key, _)| key == "email")
            .map(|(_, email)| Value::String(email.into_owned()))
            .unwrap_or(Value::Null)
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(body) => body.get("email").cloned().unwrap_or(Value::Null),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"Invalid request"})),
                );
            }
        }
    };
    let client = match EnterpriseClient::new(state.config.support.enterprise.clone()) {
        Ok(client) => client,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error":"Email signup request failed"})),
            );
        }
    };
    match client.email_signup(&email).await {
        Ok(result) => (StatusCode::OK, Json(json!(result))),
        Err(message) => (StatusCode::BAD_GATEWAY, Json(json!({"error":message}))),
    }
}

pub(super) async fn submit(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let messages = super::locale::messages(&headers);
    let success = || {
        (
            StatusCode::OK,
            Json(json!({"success":true,"message":messages[0]})),
        )
    };
    let internal = || {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":messages[3]})),
        )
    };
    let body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => return internal(),
    };
    if body.is_null() {
        return internal();
    }
    if body
        .get("alias")
        .and_then(Value::as_str)
        .is_some_and(|v| !v.trim().is_empty())
    {
        return success();
    }
    if ["name", "role", "email", "message"].iter().any(|key| {
        body.get(key)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    }) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":messages[1]})));
    }
    if !valid_email(body["email"].as_str().unwrap()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":messages[2]})));
    }
    let inquiry: Inquiry = match serde_json::from_value(body) {
        Ok(inquiry) => inquiry,
        Err(_) => return internal(),
    };
    let client = match EnterpriseClient::new(state.config.support.enterprise.clone()) {
        Ok(client) => client,
        Err(_) => return internal(),
    };
    if client.deliver(&inquiry).await.accepted() || state.config.stage == "dev" {
        return success();
    }
    internal()
}
