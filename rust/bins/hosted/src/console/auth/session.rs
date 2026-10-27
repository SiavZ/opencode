//! Purpose: decode, persist and resolve the console auth cookie and actor context.
//! Scope: reusable per-request boundary for console routes.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- axum::http::HeaderMap
//! `-- axum::response::Response
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! `-- hosted/src/console/workspace.rs
//! Invariants: cookie name and seal format match the TypeScript session; secret is redacted.
//! Contract tests: hosted/src/console/auth/tests.rs

#![allow(clippy::result_large_err)]

use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use opencode_hosted_adapters::console::auth::{ActorOutcome, LoadedSession, SessionCodec};
use opencode_hosted_adapters::console::tenant::{TenantError, TenantStore};
use opencode_hosted_domain::console::common::{Actor, ConsoleError};

use super::{ConsoleState, error, redirect};

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn codec(state: &ConsoleState) -> Result<SessionCodec, Response> {
    let secret = state
        .config
        .auth
        .session_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Console auth session secret is not configured",
            )
        })?;
    Ok(SessionCodec::new(secret))
}

/// Loads the sealed `auth` cookie, returning a fresh empty session when absent or invalid.
pub fn read_session(state: &ConsoleState, headers: &HeaderMap) -> Result<LoadedSession, Response> {
    let cookies: Vec<_> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter_map(|part| part.strip_prefix("auth="))
        })
        .collect();
    if cookies.len() > 1 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Duplicate console auth cookie",
        ));
    }
    Ok(codec(state)?.load(cookies.first().copied(), now_ms()))
}

/// Resolves an account/public/workspace actor. A fallback account selection updates the session.
pub async fn resolve_actor(
    state: &ConsoleState,
    session: &mut LoadedSession,
    workspace: Option<&str>,
    method: &Method,
) -> Result<Actor, Response> {
    let store = TenantStore::new(state.db.clone());
    match opencode_hosted_adapters::console::auth::resolve_actor(
        &store,
        &session.data,
        workspace,
        method.as_str(),
        state.config.auth.console_url.as_deref(),
    )
    .await
    {
        Ok(ActorOutcome::Actor(actor)) => Ok(actor),
        Ok(ActorOutcome::ActorWithSessionUpdate(actor, data)) => {
            session.data = data;
            Ok(actor)
        }
        Ok(ActorOutcome::Redirect(value)) => {
            Err(redirect(&value.location, value.status, value.no_store)?)
        }
        Err(TenantError::Console(e)) => {
            let status = match &e {
                ConsoleError::Validation(_) => StatusCode::BAD_REQUEST,
                ConsoleError::PermissionDenied(_) | ConsoleError::Actor(_) => StatusCode::FORBIDDEN,
                ConsoleError::NotFound(_) => StatusCode::NOT_FOUND,
                ConsoleError::Conflict(_) => StatusCode::CONFLICT,
                ConsoleError::NotYetPorted(_) => StatusCode::NOT_IMPLEMENTED,
            };
            Err(error(status, &e.to_string()))
        }
        Err(TenantError::Db(_)) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Console identity lookup failed",
        )),
    }
}

/// Adds a sealed cookie to the response, preserving existing headers and body.
pub fn with_session(
    state: &ConsoleState,
    session: &LoadedSession,
    mut response: Response,
) -> Result<Response, Response> {
    let value = codec(state)?.set_cookie(session, now_ms()).map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not seal console auth session",
        )
    })?;
    response.headers_mut().append(
        header::SET_COOKIE,
        value.parse().map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not set console auth cookie",
            )
        })?,
    );
    Ok(response)
}
