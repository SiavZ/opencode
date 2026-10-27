//! Purpose: authenticated workspace HTTP actions backed by TenantStore.
//! Scope: workspace list/create/info/update/default-region/remove REST endpoints.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! `-- hosted/src/console/mod.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/membership.rs
//! External:
//! |-- axum::{Json, Router}
//! |-- serde::Deserialize
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/state.rs
//! Invariants: workspace admission uses the shared cookie session; no SQL in handlers.
//! Contract tests: workspace::tests; console HTTP acceptance tests.

// Handlers return Axum responses directly so shared auth errors propagate unchanged.
#![allow(clippy::result_large_err)]

use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
#[cfg(test)]
use opencode_hosted_adapters::console::mail::InviteMailer;
use opencode_hosted_adapters::console::mail::{
    SesInviteMailConfig, SesInviteMailer, UnavailableInviteMailer,
};
use opencode_hosted_adapters::console::tenant::{TenantError, TenantStore};
use opencode_hosted_domain::console::common::Role;
use opencode_hosted_domain::console::common::{Actor, ConsoleError};
use opencode_hosted_domain::console::membership::{Invite, WorkspaceUpdate};
use serde::Deserialize;
use serde_json::json;
use std::fmt;
#[cfg(test)]
use std::sync::Arc;

use super::{ConsoleState, auth};

mod settings;

#[derive(Clone, Default)]
pub struct Config {
    pub mail: Option<SesInviteMailConfig>,
    #[cfg(test)]
    invite_mailer: Option<Arc<dyn InviteMailer>>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("mail_configured", &self.mail.is_some())
            .finish()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            mail: SesInviteMailConfig::from_env()?,
            #[cfg(test)]
            invite_mailer: None,
        })
    }
}

/// Explicit JSON alternatives to the baseline Solid server functions.
pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/workspaces", get(list).post(create))
        .route("/workspaces/{id}", get(info).patch(update).delete(remove))
        .route("/workspaces/{id}/default-region", post(default_region))
        .route("/workspaces/{id}/members", get(members).post(invite_member))
        .route(
            "/workspaces/{id}/members/{member_id}",
            axum::routing::patch(update_member).delete(remove_member),
        )
        .route("/workspaces/{id}/keys", get(keys))
        .route(
            "/workspaces/{id}/keys/{key_id}",
            axum::routing::delete(remove_key),
        )
        .route("/workspaces/{id}/keys", post(create_key))
        .merge(settings::routes())
}

#[derive(Deserialize)]
struct CreateInput {
    name: String,
}

#[derive(Deserialize)]
struct MemberUpdateInput {
    role: Role,
    #[serde(rename = "monthlyLimit")]
    monthly_limit: Option<i64>,
}

#[derive(Deserialize)]
struct MemberInviteInput {
    email: String,
    role: Role,
    #[serde(rename = "monthlyLimit")]
    monthly_limit: Option<i64>,
}

#[derive(Deserialize)]
struct DefaultRegionInput {
    #[serde(default, deserialize_with = "present")]
    country: Option<String>,
}

// Optional baseline fields may be omitted, but JSON null is not a valid string/bool/array.
#[derive(Deserialize)]
struct UpdateInput {
    #[serde(default, deserialize_with = "present")]
    name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    region: Option<Vec<opencode_hosted_domain::console::membership::Region>>,
    #[serde(default, deserialize_with = "present")]
    allow_training: Option<bool>,
}

fn present<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

async fn list(State(state): State<ConsoleState>, headers: HeaderMap) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, None, &Method::GET).await?;
    let Actor::Account { account_id, .. } = actor else {
        return Err(error(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    let rows = TenantStore::new(state.db.clone())
        .account_workspaces(&account_id)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(rows).into_response())
}

async fn create(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    input: Result<Json<CreateInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, None, &Method::POST).await?;
    if !matches!(actor, Actor::Account { .. }) {
        return Err(error(StatusCode::UNAUTHORIZED, "Authentication required"));
    }
    let Json(input) = input.map_err(json_error)?;
    let id = TenantStore::new(state.db.clone())
        .create_workspace(&actor, &input.name)
        .await
        .map_err(tenant_error)?;
    auth::with_session(
        &state,
        &session,
        (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
    )
}

async fn info(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let row = TenantStore::new(state.db.clone())
        .workspace_info(&id)
        .await
        .map_err(tenant_error)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Workspace not found"))?;
    auth::with_session(&state, &session, Json(row).into_response())
}

async fn update(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<UpdateInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::PATCH).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    TenantStore::new(state.db.clone())
        .update_workspace(
            &actor,
            &WorkspaceUpdate {
                name: input.name,
                region: input.region,
                allow_training: input.allow_training,
            },
        )
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn default_region(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<DefaultRegionInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    let regions = TenantStore::new(state.db.clone())
        .set_default_region(&actor, input.country.as_deref())
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(regions).into_response())
}

async fn members(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let rows = TenantStore::new(state.db.clone())
        .list_members(&actor)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(rows).into_response())
}

/// JSON REST contract: `{ email, role, monthlyLimit? }`, returns 204 on persisted invite.
async fn invite_member(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<MemberInviteInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    let invite = Invite {
        email: input.email.trim().to_string(),
        role: input.role,
        monthly_limit: input.monthly_limit,
    };
    if invite.email.is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "Email is required"));
    }
    if invite.monthly_limit.is_some_and(|limit| limit < 0) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Monthly limit must not be negative",
        ));
    }
    let store = TenantStore::new(state.db.clone());
    #[cfg(test)]
    if let Some(mailer) = &state.config.workspace.invite_mailer {
        store
            .invite_member(&actor, &invite, mailer.as_ref())
            .await
            .map_err(tenant_error)?;
        return auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response());
    }
    match &state.config.workspace.mail {
        Some(config) => match SesInviteMailer::new(config.clone()) {
            Ok(mailer) => store.invite_member(&actor, &invite, &mailer).await,
            Err(failure) => {
                tracing::error!(error = %failure, "invite mail configuration failed");
                store
                    .invite_member(&actor, &invite, &UnavailableInviteMailer)
                    .await
            }
        },
        None => {
            store
                .invite_member(&actor, &invite, &UnavailableInviteMailer)
                .await
        }
    }
    .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn keys(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let rows = TenantStore::new(state.db.clone())
        .list_keys(&actor)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(rows).into_response())
}

async fn create_key(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<CreateInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    let user_id = match &actor {
        Actor::User { user_id, .. } => user_id,
        _ => return Err(error(StatusCode::FORBIDDEN, "Workspace access denied")),
    };
    let key_id = TenantStore::new(state.db.clone())
        .create_key(&actor, user_id, &input.name)
        .await
        .map_err(tenant_error)?;
    auth::with_session(
        &state,
        &session,
        (StatusCode::CREATED, Json(json!({ "id": key_id }))).into_response(),
    )
}

async fn remove_key(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::DELETE).await?;
    require_workspace(&actor, &id)?;
    TenantStore::new(state.db.clone())
        .remove_key(&actor, &key_id)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn update_member(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path((id, member_id)): Path<(String, String)>,
    input: Result<Json<MemberUpdateInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::PATCH).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    TenantStore::new(state.db.clone())
        .update_member(&actor, &member_id, input.role, input.monthly_limit)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn remove_member(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path((id, member_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::DELETE).await?;
    require_workspace(&actor, &id)?;
    TenantStore::new(state.db.clone())
        .remove_member(&actor, &member_id)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::DELETE).await?;
    require_workspace(&actor, &id)?;
    // Workspace.remove permits any admitted user in the baseline, unlike Workspace.update.
    TenantStore::new(state.db.clone())
        .remove_workspace(&actor)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

fn require_workspace(actor: &Actor, id: &str) -> Result<(), Response> {
    match actor {
        Actor::User { workspace_id, .. } if workspace_id == id => Ok(()),
        Actor::Public {} => Err(error(StatusCode::UNAUTHORIZED, "Authentication required")),
        _ => Err(error(StatusCode::FORBIDDEN, "Workspace access denied")),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn json_error(rejection: JsonRejection) -> Response {
    error(rejection.status(), &rejection.body_text())
}

fn tenant_error(failure: TenantError) -> Response {
    let status = match &failure {
        TenantError::Console(ConsoleError::Validation(_)) => StatusCode::BAD_REQUEST,
        TenantError::Console(ConsoleError::PermissionDenied(_) | ConsoleError::Actor(_)) => {
            StatusCode::FORBIDDEN
        }
        TenantError::Console(ConsoleError::NotFound(_)) => StatusCode::NOT_FOUND,
        TenantError::Console(ConsoleError::Conflict(_)) => StatusCode::CONFLICT,
        TenantError::Console(ConsoleError::NotYetPorted(_)) => StatusCode::NOT_IMPLEMENTED,
        TenantError::Db(_) => {
            tracing::error!(error = %failure, "workspace store failure");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        }
    };
    error(status, &failure.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencode_hosted_domain::console::common::Role;

    #[test]
    fn workspace_scope_requires_matching_user() {
        let user = Actor::User {
            user_id: "usr_test".into(),
            workspace_id: "wrk_test".into(),
            account_id: "acc_test".into(),
            role: Role::Member,
        };
        assert!(require_workspace(&user, "wrk_test").is_ok());
        assert_eq!(
            require_workspace(&user, "wrk_other").unwrap_err().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            require_workspace(&Actor::Public {}, "wrk_test")
                .unwrap_err()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(
            require_workspace(
                &Actor::System {
                    workspace_id: "wrk_test".into()
                },
                "wrk_test"
            )
            .is_err()
        );
    }

    #[test]
    fn optional_fields_reject_null_and_wrong_types() {
        assert!(serde_json::from_value::<UpdateInput>(json!({})).is_ok());
        assert!(
            serde_json::from_value::<UpdateInput>(
                json!({ "allow_training": false, "region": ["cn"] })
            )
            .is_ok()
        );
        for value in [
            json!({ "name": null }),
            json!({ "region": null }),
            json!({ "allow_training": null }),
            json!({ "region": ["invalid"] }),
        ] {
            assert!(serde_json::from_value::<UpdateInput>(value).is_err());
        }
        assert!(serde_json::from_value::<DefaultRegionInput>(json!({})).is_ok());
        assert!(serde_json::from_value::<DefaultRegionInput>(json!({ "country": null })).is_err());
    }

    #[test]
    fn domain_errors_map_to_http_statuses() {
        for (failure, status) in [
            (
                ConsoleError::Validation("invalid".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ConsoleError::PermissionDenied("denied".into()),
                StatusCode::FORBIDDEN,
            ),
            (
                ConsoleError::Actor("wrong actor".into()),
                StatusCode::FORBIDDEN,
            ),
            (
                ConsoleError::NotFound("missing".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                ConsoleError::Conflict("conflict".into()),
                StatusCode::CONFLICT,
            ),
            (
                ConsoleError::NotYetPorted("missing capability".into()),
                StatusCode::NOT_IMPLEMENTED,
            ),
        ] {
            assert_eq!(tenant_error(failure.into()).status(), status);
        }
    }
}

#[cfg(test)]
#[path = "workspace_invite_tests.rs"]
mod invite_tests;
