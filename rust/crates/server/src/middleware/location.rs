//! Purpose: current-API location middlewares: bracketed `location[directory]`/`location[workspace]`
//! hints with `x-opencode-*` header fallbacks (`packages/server/src/location.ts`) and persisted
//! Session placement (`packages/server/src/middleware/session-location.ts`).
//! Scope: per-request resolution over process-level ports.
//! Uses:
//! rust/crates/
//! |-- schema/src/location.rs
//! |-- server/src/middleware/errors.rs
//! `-- server/src/services/mod.rs
//! External:
//! `-- axum::http::HeaderMap
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/dispatch.rs
//! Invariants: an empty query hint falls back to the header, then to `process.cwd()`. The
//! directory header is percent-decoded with the raw value kept on malformed escapes. Session
//! placement: an ID without the `ses` prefix answers InvalidRequestError (field sessionID), a
//! missing row SessionNotFoundError. Missing ports answer NotYetPorted, never a fake location.
//! Contract tests: server/tests/http_session.rs

use axum::http::HeaderMap;
use opencode_protocol::errors::ApiError;
use opencode_schema::ids::{SessionID, WorkspaceID};
use opencode_schema::values::AbsolutePath;

use crate::middleware::errors::HttpError;
use crate::middleware::legacy_routing::current_dir;
use crate::services::{LocationInfo, LocationRef, Services};

/// `ref(request)` from `packages/server/src/location.ts`.
/// An invalid workspace hint fails like `WorkspaceV2.ID.make` (a defect, 500).
pub fn reference(query: Option<&str>, headers: &HeaderMap) -> Result<LocationRef, HttpError> {
    let pairs: Vec<(String, String)> = query
        .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let header = |key: &str| {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let workspace = get("location[workspace]").or_else(|| header("x-opencode-workspace"));
    let directory = get("location[directory]")
        .or_else(|| header("x-opencode-directory").map(|v| decode_component(&v)))
        .unwrap_or_else(current_dir);
    Ok(LocationRef {
        directory: AbsolutePath::new(directory),
        workspace_id: workspace.map(workspace_id).transpose()?,
    })
}

fn workspace_id(value: String) -> Result<WorkspaceID, HttpError> {
    WorkspaceID::new(value).map_err(|e| HttpError::Defect(e.to_string()))
}

/// `decodeURIComponent` with the raw input kept on failure.
pub(crate) fn decode_component(input: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(input).decode_utf8();
    match decoded {
        Ok(text) if !has_bad_escape(input) => text.into_owned(),
        _ => input.to_string(),
    }
}

/// `decodeURIComponent` throws on `%` not followed by two hex digits.
fn has_bad_escape(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| {
        *b == b'%'
            && !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

/// `LocationServiceMap.get(ref)` then `Location.Info`.
pub async fn resolve(
    services: &Services,
    reference: &LocationRef,
) -> Result<LocationInfo, HttpError> {
    let Some(locations) = services.locations.as_ref() else {
        return Err(HttpError::not_yet_ported("location"));
    };
    locations
        .resolve(reference)
        .await
        .map_err(|e| HttpError::Defect(e.message))
}

/// `SessionLocationMiddleware`.
pub async fn session(
    services: &Services,
    raw_params: &[(String, String)],
) -> Result<LocationInfo, HttpError> {
    let raw = raw_params
        .iter()
        .find(|(k, _)| k == "sessionID")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let session_id = SessionID::new(raw).map_err(|_| {
        HttpError::from(ApiError::InvalidRequestError {
            message: "Invalid session ID".to_string(),
            kind: None,
            field: Some("sessionID".to_string()),
        })
    })?;
    let Some(placement) = services.placement.as_ref() else {
        return Err(HttpError::not_yet_ported("session.location"));
    };
    let row = placement
        .placement(session_id.as_str())
        .await
        .map_err(|e| HttpError::Defect(e.message))?;
    let Some((directory, workspace)) = row else {
        return Err(HttpError::from(ApiError::SessionNotFoundError {
            session_id: session_id.as_str().to_string(),
            message: format!("Session not found: {}", session_id.as_str()),
        }));
    };
    let reference = LocationRef {
        directory: AbsolutePath::new(directory),
        workspace_id: workspace
            .filter(|w| !w.is_empty())
            .map(workspace_id)
            .transpose()?,
    };
    resolve(services, &reference).await
}
