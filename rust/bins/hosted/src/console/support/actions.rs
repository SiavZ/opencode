//! Purpose: authenticate support actions and preserve baseline validation and response contracts.
//! Scope: support API handlers.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/support.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::{Json, body::Bytes, extract::State, http::{HeaderMap, StatusCode}}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: no SQL or input validation occurs before bearer authorization; DB failures are redacted.
//! Contract tests: hosted/src/console/support/tests.rs

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use opencode_hosted_adapters::console::support::{SupportStore, authorized};
use opencode_hosted_adapters::console::tenant::{TenantError, TenantStore};
use opencode_hosted_domain::console::quotas::Plan;
use serde_json::{Value, json};

use super::super::ConsoleState;

pub(super) type Reply = (StatusCode, Json<Value>);

fn input(state: &ConsoleState, headers: &HeaderMap, body: &[u8]) -> Result<Value, Reply> {
    if !authorized(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        state.config.support.api_key.as_deref(),
    ) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Unauthorized"})),
        ));
    }
    let body: Value = serde_json::from_slice(body).map_err(|_| invalid(vec![json!({"expected":"object","code":"invalid_type","path":[],"message":"Invalid input: expected object, received undefined"})]))?;
    if !body.is_object() {
        return Err(invalid(vec![
            json!({"expected":"object","code":"invalid_type","path":[],"message":format!("Invalid input: expected object, received {}",kind(Some(&body)))} ),
        ]));
    }
    Ok(body)
}

fn kind(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn invalid(issues: Vec<Value>) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"Invalid request","issues":issues})),
    )
}

fn string(body: &Value, key: &str, prefix: Option<&str>) -> Result<String, Value> {
    let Some(value) = body.get(key).and_then(Value::as_str) else {
        return Err(
            json!({"expected":"string","code":"invalid_type","path":[key],"message":format!("Invalid input: expected string, received {}",kind(body.get(key)))}),
        );
    };
    if let Some(prefix) = prefix.filter(|prefix| !value.starts_with(prefix)) {
        return Err(
            json!({"origin":"string","code":"invalid_format","format":"starts_with","prefix":prefix,"path":[key],"message":format!("Invalid string: must start with \"{prefix}\"")}),
        );
    }
    Ok(value.into())
}

pub(super) fn failure(error: TenantError) -> Reply {
    let message = match error {
        TenantError::Console(error) => error.to_string(),
        TenantError::Db(_) => "Support operation failed".into(),
    };
    (StatusCode::BAD_REQUEST, Json(json!({"error":message})))
}

async fn batch(state: ConsoleState, headers: HeaderMap, body: Bytes, blocked: bool) -> Reply {
    let body = match input(&state, &headers, &body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let Some(ids) = body.get("workspaceIDs").and_then(Value::as_array) else {
        return invalid(vec![
            json!({"expected":"array","code":"invalid_type","path":["workspaceIDs"],"message":format!("Invalid input: expected array, received {}",kind(body.get("workspaceIDs")))}),
        ]);
    };
    if ids.is_empty() {
        return invalid(vec![
            json!({"origin":"array","code":"too_small","minimum":1,"inclusive":true,"path":["workspaceIDs"],"message":"Too small: expected array to have >=1 items"}),
        ]);
    }
    let mut issues = Vec::new();
    let ids: Vec<String> = ids
        .iter()
        .enumerate()
        .filter_map(
            |(index, value)| match string(&json!({"id":value}), "id", Some("wrk_")) {
                Ok(value) => Some(value),
                Err(mut issue) => {
                    issue["path"] = json!(["workspaceIDs", index]);
                    issues.push(issue);
                    None
                }
            },
        )
        .collect();
    if !issues.is_empty() {
        return invalid(issues);
    }
    match TenantStore::new(state.db)
        .set_blocked_batch(&ids, blocked)
        .await
    {
        Ok(result) => {
            let result = if blocked {
                json!({"blocked":result.applied,"notFound":result.not_found})
            } else {
                json!({"unblocked":result.applied,"notFound":result.not_found})
            };
            (
                StatusCode::OK,
                Json(
                    json!({"success":true,"message":if blocked {"Workspaces blocked"} else {"Workspaces unblocked"},"result":result}),
                ),
            )
        }
        Err(error) => failure(error),
    }
}

pub(super) async fn block(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    batch(state, headers, body, true).await
}

pub(super) async fn unblock_batch(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    batch(state, headers, body, false).await
}

pub(super) async fn unblock(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let body = match input(&state, &headers, &body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let id = match string(&body, "workspaceID", Some("wrk_")) {
        Ok(id) => id,
        Err(issue) => return invalid(vec![issue]),
    };
    match TenantStore::new(state.db).unblock_workspace(&id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"success":true,"message":"Workspace unblocked"})),
        ),
        Err(error) => failure(error),
    }
}

pub(super) async fn delete_account(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let body = match input(&state, &headers, &body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let email = match string(&body, "email", None) {
        Ok(email) => email,
        Err(issue) => return invalid(vec![issue]),
    };
    if !support_email(&email) {
        return invalid(vec![
            json!({"origin":"string","code":"invalid_format","format":"email","pattern":r"/^(?!\.)(?!.*\.\.)([A-Za-z0-9_'+\-\.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$/","path":["email"],"message":"Invalid email address"}),
        ]);
    }
    match TenantStore::new(state.db).erase_account(&email).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"success":true,"message":"Account deleted"})),
        ),
        Err(error) => failure(error),
    }
}

pub(super) fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.chars().any(char::is_whitespace)
        && !domain.contains('@')
}

fn support_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    valid_email(value)
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(byte))
        && local
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_'+-.".contains(&byte))
        && domain.split('.').all(|label| {
            label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        && domain.rsplit('.').next().is_some_and(|label| {
            label.len() >= 2 && label.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
}

pub(super) async fn reset_quota(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let body = match input(&state, &headers, &body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let id = string(&body, "workspaceID", Some("wrk_"));
    let plan = body
        .get("plan")
        .and_then(Value::as_str)
        .and_then(|v| Plan::parse(v).ok());
    let mut issues = Vec::new();
    if let Err(issue) = &id {
        issues.push(issue.clone());
    }
    if plan.is_none() {
        issues.push(json!({"code":"invalid_value","values":["lite","subscription"],"path":["plan"],"message":"Invalid option: expected one of \"lite\"|\"subscription\""}));
    }
    if !issues.is_empty() {
        return invalid(issues);
    }
    match SupportStore::new(state.db)
        .reset_quota(&id.unwrap(), plan.unwrap())
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({"success":true,"message":"Usage counters reset to zero","result":result})),
        ),
        Err(error) => failure(error),
    }
}

pub(super) async fn create_referral(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let body = match input(&state, &headers, &body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let inviter = string(&body, "inviterWorkspaceID", Some("wrk_"));
    let invitee = string(&body, "inviteeWorkspaceID", Some("wrk_"));
    let issues = [&inviter, &invitee]
        .iter()
        .filter_map(|value| value.as_ref().err().cloned())
        .collect::<Vec<_>>();
    if !issues.is_empty() {
        return invalid(issues);
    }
    match SupportStore::new(state.db)
        .create_referral(&inviter.unwrap(), &invitee.unwrap())
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({"success":true,"message":"Referral created","result":result})),
        ),
        Err(error) => failure(error),
    }
}
