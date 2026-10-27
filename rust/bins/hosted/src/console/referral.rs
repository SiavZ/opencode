//! Purpose: expose referral summary, quota preview, reward application and cookie attribution.
//! Scope: authenticated HTTP alternatives to go-referral Solid server functions.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/referral.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! |-- axum::Router
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: baseline permits workspace members, not just admins; mutations use shared CSRF checks.
//! Contract tests: hosted/src/console/referral/tests.rs

#![allow(clippy::result_large_err)]

use axum::extract::{Path, Query, State, rejection::JsonRejection};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use opencode_hosted_adapters::console::referral::{ReferralError, ReferralStore};
use opencode_hosted_domain::console::common::{Actor, ConsoleError};
use opencode_hosted_domain::console::referral::normalize_code;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ConsoleState, auth};

const CLEAR_COOKIE: &str = "oc_referral=; Path=/; Max-Age=0; SameSite=Lax; HttpOnly";

pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/workspaces/{id}/referral", get(summary))
        .route("/workspaces/{id}/referral/preview", get(preview))
        .route("/workspaces/{id}/referral/apply", post(apply))
        .route("/referral/capture", post(capture))
}

#[derive(Deserialize)]
struct RewardInput {
    #[serde(rename = "referralID")]
    referral_id: String,
}

#[derive(Deserialize)]
struct PreviewInput {
    #[serde(rename = "referralID")]
    referral_id: Option<String>,
}

#[derive(Deserialize)]
struct CaptureInput {
    code: Option<String>,
}

async fn summary(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    let account = account(&actor, Some(&id))?;
    let store = ReferralStore::new(state.db.clone());
    if cookie_code(&headers).is_some() {
        auth::require_same_origin(&state, &headers)?;
    }
    let clear = attribute_cookie(&store, account, &headers).await;
    let result = store.summary(&id, account).await.map_err(referral_error);
    let response = match result {
        Ok(result) => auth::with_session(&state, &session, Json(result).into_response())?,
        Err(response) => return Err(with_clear_cookie(response, clear)),
    };
    Ok(with_clear_cookie(response, clear))
}

async fn preview(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(input): Query<PreviewInput>,
) -> Result<Response, Response> {
    // Solid query returns null before resolving an actor if referralID is omitted.
    let Some(referral) = input.referral_id.filter(|value| !value.is_empty()) else {
        return Ok(Json(Value::Null).into_response());
    };
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    account(&actor, Some(&id))?;
    let limits = state.config.zen.billing_limits().unwrap_or(&Value::Null);
    let result = ReferralStore::new(state.db.clone())
        .usage_preview(&id, &referral, limits)
        .await
        .map_err(referral_error)?;
    auth::with_session(&state, &session, Json(result).into_response())
}

async fn apply(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<RewardInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    account(&actor, Some(&id))?;
    let Json(input) = input.map_err(json_error)?;
    let result = ReferralStore::new(state.db.clone())
        .apply_reward(&id, &input.referral_id)
        .await
        .map_err(referral_error)?;
    auth::with_session(&state, &session, Json(result).into_response())
}

async fn capture(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    input: Result<Json<CaptureInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, None, &Method::POST).await?;
    let account = account(&actor, None)?;
    let Json(input) = input.map_err(json_error)?;
    let store = ReferralStore::new(state.db.clone());
    let code = normalize_code(input.code.as_deref()).or_else(|| cookie_code(&headers));
    if let Some(code) = &code
        && let Err(failure) = store.create_from_account(account, Some(code)).await
    {
        // Baseline createReferralFromCookie logs attribution failures and still clears the cookie.
        tracing::warn!(error = %failure, "referral attribution failed");
    }
    let response = auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())?;
    Ok(with_clear_cookie(response, code.is_some()))
}

async fn attribute_cookie(store: &ReferralStore, account: &str, headers: &HeaderMap) -> bool {
    let Some(code) = cookie_code(headers) else {
        return false;
    };
    if let Err(failure) = store.create_from_account(account, Some(&code)).await {
        tracing::warn!(error = %failure, "referral attribution failed");
    }
    true
}

fn cookie_code(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .find_map(|value| value.strip_prefix("oc_referral="))?;
    // Baseline normalizes the raw cookie value; it does not URL-decode it first.
    normalize_code(Some(value))
}

fn with_clear_cookie(mut response: Response, clear: bool) -> Response {
    if clear {
        response.headers_mut().append(
            header::SET_COOKIE,
            header::HeaderValue::from_static(CLEAR_COOKIE),
        );
    }
    response
}

fn account<'a>(actor: &'a Actor, workspace: Option<&str>) -> Result<&'a str, Response> {
    match actor {
        Actor::User {
            account_id,
            workspace_id,
            ..
        } if workspace == Some(workspace_id.as_str()) => Ok(account_id),
        Actor::Account { account_id, .. } if workspace.is_none() => Ok(account_id),
        _ => Err(error(StatusCode::FORBIDDEN, "Workspace access denied")),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn json_error(rejection: JsonRejection) -> Response {
    error(rejection.status(), &rejection.body_text())
}
fn referral_error(failure: ReferralError) -> Response {
    let status = match &failure {
        ReferralError::Console(ConsoleError::Validation(_)) => StatusCode::BAD_REQUEST,
        ReferralError::Console(ConsoleError::PermissionDenied(_) | ConsoleError::Actor(_)) => {
            StatusCode::FORBIDDEN
        }
        ReferralError::Console(ConsoleError::NotFound(_)) => StatusCode::NOT_FOUND,
        ReferralError::Console(ConsoleError::Conflict(_)) => StatusCode::CONFLICT,
        ReferralError::Console(ConsoleError::NotYetPorted(_)) => StatusCode::NOT_IMPLEMENTED,
        _ => {
            tracing::error!(error = %failure, "referral repository failure");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        }
    };
    error(status, &failure.to_string())
}

#[cfg(test)]
mod tests;
