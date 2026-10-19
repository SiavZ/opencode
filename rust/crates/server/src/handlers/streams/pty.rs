//! Purpose: current and legacy PTY WebSocket connection handlers and shared ticket issuance.
//! Scope: per-connection transport, after current placement or legacy instance routing.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/location.rs
//! |-- server/src/services/streams.rs
//! `-- server/src/transport/websocket.rs
//! External:
//! |-- axum::body::Body
//! |-- axum::extract::Path
//! |-- axum::extract::Request
//! |-- axum::extract::State
//! |-- axum::extract::ws::WebSocketUpgrade
//! |-- axum::extract::ws::rejection::WebSocketUpgradeRejection
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::http::header
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- form_urlencoded::parse
//! |-- header::HOST
//! |-- header::ORIGIN
//! |-- location::reference
//! |-- location::resolve
//! |-- str::to_string
//! |-- websocket::close_with
//! `-- websocket::run
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/streams.rs
//! Invariants: existence precedes ticket/origin rejection; legacy requires a running PTY.
//! Socket registration, replay cursor parsing and close reasons retain both API contracts.
//! Contract tests: server/tests/ws_pty.rs; cursor parsing unit test below.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opencode_platform_api::pty::{PtyError, PtyStatus, ReplayFrom};

use crate::app::AppState;
use crate::middleware::cors::is_allowed_request_origin;
use crate::middleware::errors::HttpError;
use crate::middleware::location;
use crate::services::streams::TicketScope;
use crate::transport::websocket;

/// `Number(x)` for a query value, then `isSafeInteger && >= -1`.
pub fn parse_cursor(raw: Option<&str>) -> Option<i64> {
    let raw = raw?;
    let trimmed = raw.trim();
    let number = if trimmed.is_empty() {
        0.0
    } else if trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
    {
        trimmed.parse::<f64>().ok()?
    } else {
        return None;
    };
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    (number.fract() == 0.0 && number.abs() <= MAX_SAFE && number >= -1.0).then_some(number as i64)
}

fn query_value(query: Option<&str>, name: &str) -> Option<String> {
    query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    })
}

fn empty(status: StatusCode) -> Response {
    status.into_response()
}

/// Inputs shared by both PTY connect routes once placement is known.
struct Connect {
    pty_id: String,
    directory: String,
    workspace_id: Option<String>,
    /// Current route closes with `session exited` for exited PTYs; legacy says `session not found`.
    exited_reason: &'static str,
    /// Legacy treats an exited PTY as missing (404 before upgrade).
    require_running: bool,
}

pub(super) async fn current_pty(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let query = request.uri().query().map(str::to_string);
    let reference = match location::reference(query.as_deref(), &headers) {
        Ok(reference) => reference,
        Err(error) => return error.into_response(),
    };
    let info = match location::resolve(&state.services, &reference).await {
        Ok(info) => info,
        Err(error) => return error.into_response(),
    };
    let connect = Connect {
        pty_id,
        directory: info.directory.as_str().to_string(),
        workspace_id: info.workspace_id.map(|w| w.into_inner()),
        exited_reason: "session exited",
        require_running: false,
    };
    pty_connect(state, connect, upgrade, headers, query).await
}

pub(super) async fn legacy_pty(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().map(str::to_string);
    let mut routed = Request::new(Body::empty());
    *routed.uri_mut() = request.uri().clone();
    *routed.headers_mut() = headers.clone();
    let context = match super::legacy_context(&state, routed).await {
        Ok(context) => context,
        Err(response) => return *response,
    };
    let connect = Connect {
        pty_id,
        directory: context.directory,
        workspace_id: context.workspace_id,
        exited_reason: "session not found",
        require_running: true,
    };
    pty_connect(state, connect, upgrade, headers, query).await
}

async fn pty_connect(
    state: AppState,
    connect: Connect,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    headers: HeaderMap,
    query: Option<String>,
) -> Response {
    let streams = state.services.streams.clone();
    let Some(provider) = streams.pty.clone() else {
        return HttpError::not_yet_ported("pty.connect").into_response();
    };
    let service = match provider
        .pty(&connect.directory, connect.workspace_id.as_deref())
        .await
    {
        Ok(service) => service,
        Err(error) => return HttpError::Defect(error.message).into_response(),
    };
    let exists = match service.get(&connect.pty_id).await {
        Ok(info) => !connect.require_running || info.status == PtyStatus::Running,
        Err(_) => false,
    };
    if !exists {
        return empty(StatusCode::NOT_FOUND);
    }
    if let Some(ticket) = query_value(query.as_deref(), "ticket").filter(|t| !t.is_empty()) {
        let cors = state.config.cors.clone().unwrap_or_default();
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
        let scope = TicketScope {
            pty_id: connect.pty_id.clone(),
            directory: Some(connect.directory.clone()),
            workspace_id: connect.workspace_id.clone(),
        };
        let valid = is_allowed_request_origin(origin, host, &cors)
            && streams.tickets.consume(&ticket, &scope);
        if !valid {
            return empty(StatusCode::FORBIDDEN);
        }
    }
    let from = match parse_cursor(query_value(query.as_deref(), "cursor").as_deref()) {
        None => ReplayFrom::Start,
        Some(-1) => ReplayFrom::End,
        Some(n) => ReplayFrom::Cursor(n as u64),
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        // `ctx.request.upgrade` with `orDie`: a non-upgrade request is a defect.
        Err(rejection) => return HttpError::Defect(rejection.to_string()).into_response(),
    };
    let registration = streams.sockets.register();
    let pty_id = connect.pty_id.clone();
    let exited_reason = connect.exited_reason;
    upgrade.on_upgrade(move |socket| async move {
        let Some((_token, closing)) = registration else {
            websocket::close_with(socket, 1001, "server closing").await;
            return;
        };
        match service.attach(&pty_id, from).await {
            Ok(attachment) => websocket::run(socket, attachment, closing).await,
            Err(PtyError::Exited(_)) => websocket::close_with(socket, 4404, exited_reason).await,
            Err(_) => websocket::close_with(socket, 4404, "session not found").await,
        }
    })
}

/// Shared ticket issuer used by the `connect-token` handlers.
pub fn issue_ticket(
    state: &AppState,
    scope: TicketScope,
) -> crate::services::streams::ConnectToken {
    let tickets: Arc<dyn crate::services::streams::PtyTickets> =
        state.services.streams.tickets.clone();
    tickets.issue(scope)
}

#[cfg(test)]
mod tests {
    use super::parse_cursor;

    #[test]
    fn cursor_parsing_matches_number_semantics() {
        assert_eq!(parse_cursor(None), None);
        assert_eq!(parse_cursor(Some("")), Some(0));
        assert_eq!(parse_cursor(Some("-1")), Some(-1));
        assert_eq!(parse_cursor(Some("-2")), None);
        assert_eq!(parse_cursor(Some("1.5")), None);
        assert_eq!(parse_cursor(Some("abc")), None);
        assert_eq!(parse_cursor(Some("1e3")), Some(1000));
    }
}
