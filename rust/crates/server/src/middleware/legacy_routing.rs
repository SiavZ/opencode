//! Purpose: legacy instance/workspace routing (`httpapi/middleware/workspace-routing.ts` and
//! `server/shared/workspace-routing.ts`): pick local directory/workspace context or forward the
//! request to a remote workspace target.
//! Scope: per-request routing plan over process-level workspace services.
//! Uses:
//! rust/crates/
//! |-- server/src/middleware/errors.rs
//! |-- server/src/services/mod.rs
//! `-- server/src/transport/proxy.rs
//! External:
//! `-- axum::http::Request
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/dispatch.rs
//! Invariants: session placement wins over `workspace` query; `/api/` paths reject malformed
//! workspace IDs with InvalidRequestError (kind Query, field workspace); unknown workspaces
//! answer 500 text `Workspace not found: <id>`; remote targets require an active sync
//! connection (503 text otherwise); `/experimental/workspace*`, `GET /session` and `/console*`
//! stay local; `OPENCODE_WORKSPACE_ID` disables forwarding.
//! Contract tests: server/tests/http_proxy.rs

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use opencode_protocol::errors::ApiError;

use crate::middleware::errors::{HttpError, text};
use crate::services::{Services, WorkspaceTarget};
use crate::transport::proxy;

/// Resolved local context for a legacy instance request (`WorkspaceRouteContext`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteContext {
    pub directory: String,
    pub workspace_id: Option<String>,
}

/// Either serve locally with a context or the request was answered (proxied or rejected).
pub enum Plan {
    Local(RouteContext),
    Answered(axum::response::Response),
}

/// `isLocalWorkspaceRoute`.
pub fn is_local_workspace_route(method: &Method, path: &str) -> bool {
    let rules: [(Option<Method>, &str, bool); 3] = [
        (None, "/experimental/workspace", true),
        (None, "/session/status", false),
        (Some(Method::GET), "/session", true),
    ];
    for (rule_method, rule_path, local) in rules {
        if rule_method.as_ref().is_some_and(|m| m != method) {
            continue;
        }
        if path == rule_path || path.starts_with(&format!("{rule_path}/")) {
            return local;
        }
    }
    false
}

/// `getWorkspaceRouteSessionID`.
pub fn route_session_id(path: &str) -> Option<String> {
    if path == "/session/status" {
        return None;
    }
    if let Some(rest) = path.strip_prefix("/session/") {
        let id = rest.split('/').next().unwrap_or("");
        return (!id.is_empty()).then(|| id.to_string());
    }
    let rest = path.strip_prefix("/experimental/session/")?;
    let id = rest.strip_suffix("/background")?;
    (!id.is_empty() && !id.contains('/')).then(|| id.to_string())
}

fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, v)| k == name && !v.is_empty())
            .map(|(_, v)| v.into_owned())
    })
}

/// Plan and, for remote targets, execute the request.
#[allow(clippy::too_many_arguments)]
pub async fn route(
    services: &Services,
    client: &reqwest::Client,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Body,
) -> Result<(Plan, Option<Body>), HttpError> {
    let workspaces = services.workspaces.as_ref();
    let session = match (route_session_id(path), workspaces) {
        (Some(id), Some(ws)) => ws.session(&id).await,
        _ => None,
    };
    let env_workspace = workspaces.and_then(|w| w.configured_workspace_id());
    let param = query_param(query, "workspace");
    let session_workspace = session.as_ref().and_then(|s| s.1.clone());
    if path.starts_with("/api/")
        && session_workspace.is_none()
        && param.as_deref().is_some_and(|p| !p.starts_with("wrk"))
    {
        return Err(HttpError::from(ApiError::InvalidRequestError {
            message: "Invalid workspace query parameter".to_string(),
            kind: Some("Query".to_string()),
            field: Some("workspace".to_string()),
        }));
    }
    let workspace_id = session_workspace.or(param);
    let workspace = match (&workspace_id, &env_workspace, workspaces) {
        (Some(id), None, Some(ws)) => ws.workspace(id).await,
        (Some(_), None, None) => return Err(HttpError::not_yet_ported("workspace.routing")),
        _ => None,
    };
    if let (Some(id), None, None) = (&workspace_id, &workspace, &env_workspace) {
        let response = text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            format!("Workspace not found: {id}"),
        );
        return Ok((Plan::Answered(response), Some(body)));
    }
    let stays_local = is_local_workspace_route(method, path) || path.starts_with("/console");
    if let (Some(workspace), None, false, Some(ws)) =
        (&workspace, &env_workspace, stays_local, workspaces)
    {
        match ws.target(workspace).await {
            WorkspaceTarget::Local { directory } => {
                let context = RouteContext {
                    directory,
                    workspace_id: Some(workspace.id.clone()),
                };
                return Ok((Plan::Local(context), Some(body)));
            }
            WorkspaceTarget::Remote {
                url,
                headers: extra,
            } => {
                if !ws.is_syncing(&workspace.id).await {
                    let response = text(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "text/plain; charset=utf-8",
                        format!("broken sync connection for workspace: {}", workspace.id),
                    );
                    return Ok((Plan::Answered(response), Some(body)));
                }
                let is_upgrade = headers
                    .get(axum::http::header::UPGRADE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
                if is_upgrade {
                    return Err(HttpError::not_yet_ported("workspace.proxy.websocket"));
                }
                let target = proxy::proxy_url(&url, path, query).map_err(HttpError::Defect)?;
                let response =
                    proxy::http(client, target, &extra, method.clone(), headers, body).await;
                return Ok((Plan::Answered(response), None));
            }
        }
    }
    let directory = session
        .as_ref()
        .map(|s| s.0.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| query_param(query, "directory"))
        .or_else(|| {
            headers
                .get("x-opencode-directory")
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(current_dir);
    let context = RouteContext {
        directory,
        workspace_id: env_workspace.or(workspace_id),
    };
    Ok((Plan::Local(context), Some(body)))
}

/// `process.cwd()`.
pub fn current_dir() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}
