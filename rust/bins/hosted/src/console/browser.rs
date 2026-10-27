//! Purpose: delegate locally compiled SolidStart server actions to authenticated Rust routes.
//! Scope: browser JSON boundary; SolidStart retains RPC serialization and rendering.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/billing.rs
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/console/workspace.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- axum::{Json, Router}
//! |-- serde::Deserialize
//! |-- serde_json::json
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: every delegated route retains its auth and origin checks; no arbitrary URI dispatch.
//! Contract tests: browser/tests.rs; rust/scripts/console-browser/acceptance.test.ts

#![allow(clippy::result_large_err)]

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::{Actor, Role};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{ConsoleState, auth, billing, public, referral, support, workspace};

#[cfg(test)]
mod tests;

pub fn routes() -> Router<ConsoleState> {
    Router::new().route("/_console/browser", post(dispatch))
}

#[derive(Deserialize)]
struct Call {
    operation: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    page: Option<u32>,
}

fn segment(value: Option<&str>) -> Result<&str, Response> {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 255
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid browser route identifier"))
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

fn target(call: &Call) -> Result<(Method, String), Response> {
    if call.operation == "go.referral.capture" {
        return Ok((Method::POST, "/referral/capture".into()));
    }
    if call.operation == "getBenchmarks" {
        return Ok((Method::GET, "/benchmarks".into()));
    }
    if call.operation == "changelog" {
        return Ok((Method::GET, "/changelog.json".into()));
    }
    if call.operation == "github" {
        return Ok((Method::GET, "/github.json".into()));
    }
    if call.operation == "emailSignup" {
        return Ok((Method::POST, "/api/email-signup".into()));
    }
    if call.operation == "auth.logout" {
        return Ok((Method::GET, "/auth/logout".into()));
    }
    if call.operation == "getTaskDetail" {
        let id = segment(call.workspace.as_deref())?;
        let task = call
            .resource
            .as_deref()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid task identifier"))?;
        let task: String = url::form_urlencoded::byte_serialize(task.as_bytes()).collect();
        return Ok((Method::GET, format!("/benchmarks/{id}/tasks/{task}")));
    }
    if call.operation == "workspaces" {
        return Ok((Method::GET, "/workspaces".into()));
    }
    if call.operation == "workspace.create" {
        return Ok((Method::POST, "/workspaces".into()));
    }
    let workspace = segment(call.workspace.as_deref())?;
    let base = format!("/workspaces/{workspace}");
    let (method, suffix) = match call.operation.as_str() {
        "go.referral.get" => (Method::GET, "/referral".into()),
        "go.referral.reward.apply" => (Method::POST, "/referral/apply".into()),
        "go.referral.usagePreview" => {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            if let Some(id) = call.input.get("referralID").and_then(Value::as_str) {
                query.append_pair("referralID", id);
            }
            (Method::GET, format!("/referral/preview?{}", query.finish()))
        }
        "workspace.get" => (Method::GET, String::new()),
        "workspace.update" => (Method::PATCH, String::new()),
        "key.list" => (Method::GET, "/keys".into()),
        "key.create" => (Method::POST, "/keys".into()),
        "key.remove" => (
            Method::DELETE,
            format!("/keys/{}", segment(call.resource.as_deref())?),
        ),
        "member.list" => (Method::GET, "/members".into()),
        "member.create" => (Method::POST, "/members".into()),
        "member.update" => (
            Method::PATCH,
            format!("/members/{}", segment(call.resource.as_deref())?),
        ),
        "member.remove" => (
            Method::DELETE,
            format!("/members/{}", segment(call.resource.as_deref())?),
        ),
        "billing.get" => (Method::GET, "/billing".into()),
        "payment.list" => (Method::GET, "/billing/payments".into()),
        "usage.list" => (
            Method::GET,
            format!("/billing/usage?page={}&limit=50", call.page.unwrap_or(0)),
        ),
        "checkoutUrl" => (Method::POST, "/billing/checkout".into()),
        "sessionUrl" | "liteSessionUrl" => (Method::POST, "/billing/portal".into()),
        "receipt.download" => (Method::POST, "/billing/receipt".into()),
        "billing.reload" => (Method::POST, "/billing/reload".into()),
        "billing.setReload" => (Method::PATCH, "/billing/reload".into()),
        "billing.setMonthlyLimit" => (Method::POST, "/billing/monthly-limit".into()),
        "billing.redeemCoupon" => (Method::POST, "/billing/redeem".into()),
        "subscription.get" => (Method::GET, "/billing/subscription".into()),
        "cancelWaitlist" => (Method::POST, "/billing/subscription/cancel-waitlist".into()),
        "enroll" => (Method::POST, "/billing/subscription/enroll".into()),
        "setUseBalance" => (Method::PATCH, "/billing/subscription/use-balance".into()),
        "lite.subscription.get" => (Method::GET, "/billing/lite".into()),
        "lite.subscription.usage" => {
            let window = call
                .input
                .get("window")
                .and_then(Value::as_str)
                .filter(|s| matches!(*s, "rolling" | "weekly" | "monthly"))
                .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid usage window"))?;
            (Method::GET, format!("/billing/lite/usage?window={window}"))
        }
        "liteCheckoutUrl" => (Method::POST, "/billing/lite/checkout".into()),
        "setLiteUseBalance" => (Method::PATCH, "/billing/lite/use-balance".into()),
        "go.providerRouting.set" | "go.allowTraining.set" => (Method::PATCH, String::new()),
        "provider.list" => return Ok((Method::GET, format!("{base}/providers"))),
        "provider.save" => return Ok((Method::POST, format!("{base}/providers"))),
        "provider.remove" => {
            return Ok((
                Method::DELETE,
                format!("{base}/providers/{}", segment(call.resource.as_deref())?),
            ));
        }
        "model.info" => return Ok((Method::GET, format!("{base}/models"))),
        "model.toggle" => return Ok((Method::POST, format!("{base}/models/toggle"))),
        "getCosts" => {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            for name in ["year", "month", "tzOffset"] {
                let value = call
                    .input
                    .get(name)
                    .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Missing graph argument"))?;
                query.append_pair(
                    name,
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                        .as_str(),
                );
            }
            return Ok((
                Method::GET,
                format!("{base}/usage/graph?{}", query.finish()),
            ));
        }
        _ => {
            return Err(failure(
                StatusCode::NOT_IMPLEMENTED,
                "Browser operation is not yet ported",
            ));
        }
    };
    Ok((method, format!("{base}{suffix}")))
}

async fn dispatch(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Json(call): Json<Call>,
) -> Result<Response, Response> {
    // This is public same-origin transport, never a privileged internal bypass.
    auth::require_same_origin(&state, &headers)?;
    if call.operation == "black.paused" {
        return Ok(Json(state.config.stage == "production").into_response());
    }
    if matches!(
        call.operation.as_str(),
        "session.get" | "member.actor" | "userEmail" | "lastWorkspace"
    ) {
        return identity(&state, &headers, &call).await;
    }
    let (method, uri) = target(&call)?;
    let mut request = Request::builder().method(method).uri(uri);
    for name in ["cookie", "origin", "referer", "host"] {
        for value in headers.get_all(name) {
            request = request.header(name, value);
        }
    }
    let body = if call.input.is_null() {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from(call.input.to_string())
    };
    let request = request
        .body(body)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Invalid browser request"))?;
    let router = Router::new()
        .merge(workspace::routes())
        .merge(billing::routes())
        .merge(auth::routes())
        .merge(public::routes())
        .merge(support::routes())
        .merge(referral::routes())
        .with_state(state);
    let response = router.oneshot(request).await.unwrap();
    Ok(response)
}

async fn identity(
    state: &ConsoleState,
    headers: &HeaderMap,
    call: &Call,
) -> Result<Response, Response> {
    let mut session = auth::read_session(state, headers)?;
    let workspace = if call.operation == "lastWorkspace" {
        None
    } else {
        Some(segment(call.workspace.as_deref())?)
    };
    let actor = auth::resolve_actor(state, &mut session, workspace, &Method::GET).await?;
    let store = TenantStore::new(state.db.clone());
    let value = match (&*call.operation, actor) {
        (
            "session.get" | "member.actor",
            Actor::User {
                role,
                workspace_id,
                user_id,
                ..
            },
        ) => json!({
            "isAdmin": role == Role::Admin,
            "isBeta": state.config.stage != "production" || workspace_id == "wrk_01K46JDFR0E75SG2Q8K172KF3Y",
            "actorID": user_id,
            "actorRole": role
        }),
        ("userEmail", actor @ Actor::User { .. }) => serde_json::to_value(
            store
                .member_auth_email(
                    &actor,
                    match &actor {
                        Actor::User { user_id, .. } => user_id,
                        _ => unreachable!(),
                    },
                )
                .await
                .map_err(|_| {
                    failure(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "User email lookup failed",
                    )
                })?,
        )
        .unwrap(),
        ("lastWorkspace", Actor::Account { account_id, .. }) => {
            serde_json::to_value(store.last_seen_workspace(&account_id).await.map_err(|_| {
                failure(StatusCode::INTERNAL_SERVER_ERROR, "Workspace lookup failed")
            })?)
            .unwrap()
        }
        _ => return Err(failure(StatusCode::UNAUTHORIZED, "Authentication required")),
    };
    auth::with_session(state, &session, Json(value).into_response())
}
