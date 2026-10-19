//! Purpose: streaming endpoint handlers: current `/api/event`, durable `/api/session/:id/event`,
//! legacy `/event` and `/global/event`, and the PTY WebSocket connect routes (current
//! `/api/pty/:id/connect`, legacy `/pty/:id/connect`). Mounted by `app.rs` through `routes()`;
//! `owns()` tells the generic dispatcher to skip these operations.
//! Scope: per-connection handlers.
//! Uses:
//! rust/crates/
//! |-- protocol/src/operation.rs
//! |-- protocol/src/sse.rs
//! |-- schema/src/session_event.rs
//! |-- server/src/app.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/middleware/decode.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! |-- server/src/middleware/location.rs
//! |-- server/src/services/streams.rs
//! |-- server/src/transport/sse.rs
//! `-- server/src/transport/websocket.rs
//! External:
//! |-- axum::extract::ws::WebSocketUpgrade
//! `-- futures_util::stream::StreamExt
//! Used by:
//! rust/crates/
//! |-- server/src/app.rs
//! `-- server/src/handlers/mod.rs
//! Invariants: current `/api/event` writes `server.connected` first, the 15 s `: heartbeat`
//! comment (first tick immediate, as in Effect `Stream.tick`), and ends the body abnormally when the
//! 256-slot subscriber overflows. Legacy streams write `{id,type,properties}` frames with a 10 s
//! `server.heartbeat` (first tick dropped). `/event` filters by instance directory/workspace and
//! ends after `server.instance.disposed`. Session events check placement (404 before streaming),
//! then replay durable events strictly after `after` and continue live. A stream failure is
//! written as the Effect `effect/httpapi/stream/failure` event. PTY connect checks existence (empty
//! 404) before ticket/origin (empty 403), then upgrades. Sockets are tracked by `SocketTracker`.
//! Contract tests: server/tests/sse_events.rs, server/tests/ws_pty.rs

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, BoxStream, StreamExt};
use opencode_platform_api::pty::{PtyError, PtyStatus, ReplayFrom};
use opencode_protocol::sse::{
    CONNECTED_TYPE, CURRENT_HEARTBEAT, CURRENT_HEARTBEAT_INTERVAL, CURRENT_SUBSCRIBER_CAPACITY,
    INSTANCE_DISPOSED_TYPE, LEGACY_HEARTBEAT_INTERVAL, LEGACY_HEARTBEAT_TYPE,
};
use opencode_schema::ids::EventID;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::middleware::cors::is_allowed_request_origin;
use crate::middleware::errors::HttpError;
use crate::middleware::legacy_routing::{self, Plan};
use crate::middleware::{decode, location};
use crate::services::streams::TicketScope;
use crate::transport::{sse, websocket};

/// Effect reserved SSE failure event name.
pub const STREAM_FAILURE_EVENT: &str = "effect/httpapi/stream/failure";

/// Paths this module mounts itself (current and legacy registry templates).
const OWNED: [&str; 6] = [
    "/api/event",
    "/api/session/:sessionID/event",
    "/api/pty/:ptyID/connect",
    "/event",
    "/global/event",
    "/pty/:ptyID/connect",
];

/// True when this module mounts the operation at `path` (registry template).
pub fn owns(path: &str) -> bool {
    OWNED.contains(&path)
}

/// Stream routes merged into the app router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/event", get(current_events))
        .route("/api/session/{sessionID}/event", get(session_events))
        .route("/api/pty/{ptyID}/connect", get(current_pty))
        .route("/event", get(legacy_events))
        .route("/global/event", get(global_events))
        .route("/pty/{ptyID}/connect", get(legacy_pty))
}

fn event_id() -> String {
    EventID::create().into_inner()
}

async fn current_events(State(state): State<AppState>) -> Response {
    let Some(events) = state.services.streams.events.clone() else {
        return HttpError::not_yet_ported("event.subscribe").into_response();
    };
    // Subscribe before the response is returned so nothing published afterwards is lost.
    let live = events.all_bounded(CURRENT_SUBSCRIBER_CAPACITY);
    let connected = json!({ "id": event_id(), "type": CONNECTED_TYPE, "data": {} });
    let frames = stream::once(async move { Ok(sse::data(&connected.to_string())) })
        .chain(live.map(|item| match item {
            Ok(payload) => Ok(sse::data(
                &serde_json::to_string(&payload).unwrap_or_default(),
            )),
            Err(error) => Err(std::io::Error::other(error.to_string())),
        }))
        .boxed();
    let heartbeat = sse::ticks(CURRENT_HEARTBEAT_INTERVAL, 0, || {
        CURRENT_HEARTBEAT.to_string()
    });
    sse::fallible_response(sse::halt_left_fallible(frames, heartbeat))
}

async fn session_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    request: Request,
) -> Response {
    let raw = vec![("sessionID".to_string(), session_id.clone())];
    if let Err(error) = location::session(&state.services, &raw).await {
        return error.into_response();
    }
    let api = opencode_protocol::current::api();
    let query = request.uri().query().map(str::to_string);
    let after = match api.find("session.events") {
        Some(endpoint) => match decode::query(api, &endpoint.query, query.as_deref()) {
            Ok(decoded) => decoded
                .get("after")
                .and_then(Value::as_f64)
                .map(|n| n as i64),
            Err(reason) => return HttpError::schema(true, "Query", &reason).into_response(),
        },
        None => None,
    };
    let Some(events) = state.services.streams.events.clone() else {
        return HttpError::not_yet_ported("session.events").into_response();
    };
    let durable_types: Vec<&'static str> = opencode_schema::session_event::DURABLE_DEFINITIONS
        .iter()
        .map(|d| d.r#type)
        .collect();
    let frames = events
        .durable(&session_id, after)
        .filter_map(move |item| {
            let out = match item {
                Ok(payload)
                    if payload.durable.is_some()
                        && durable_types.contains(&payload.event_type.as_str()) =>
                {
                    Some(Ok(sse::data(
                        &serde_json::to_string(&payload).unwrap_or_default(),
                    )))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error.to_string())),
            };
            futures_util::future::ready(out)
        })
        .scan(false, |failed, item| {
            // After a failure frame the stream ends (`Stream.catchCause` then completion).
            if *failed {
                return futures_util::future::ready(None);
            }
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(message) => {
                    *failed = true;
                    failure_frame(&message)
                }
            };
            futures_util::future::ready(Some(chunk))
        })
        .boxed();
    sse::response(frames)
}

/// `Stream.orDie` defects encoded through `Schema.Cause(Never, Defect)`.
fn failure_frame(message: &str) -> String {
    let cause = json!([{ "_tag": "Die", "defect": { "name": "Error", "message": message } }]);
    opencode_protocol::sse::Frame {
        id: None,
        event: STREAM_FAILURE_EVENT.to_string(),
        data: cause.to_string(),
    }
    .encode()
}

/// Legacy instance context for streaming routes. `Err` carries an already-built response
/// (proxied, rejected or not-ported).
async fn legacy_context(
    state: &AppState,
    request: Request,
) -> Result<legacy_routing::RouteContext, Box<Response>> {
    let (parts, body) = request.into_parts();
    let routed = legacy_routing::route(
        &state.services,
        &state.client,
        &parts.method,
        parts.uri.path(),
        parts.uri.query(),
        &parts.headers,
        body,
    )
    .await;
    match routed {
        Ok((Plan::Local(context), _)) => legacy_routing::instance(context)
            .await
            .map_err(|error| Box::new(error.into_response())),
        Ok((Plan::Answered(response), _)) => Err(Box::new(response)),
        Err(error) => Err(Box::new(error.into_response())),
    }
}

async fn legacy_events(State(state): State<AppState>, request: Request) -> Response {
    let context = match legacy_context(&state, request).await {
        Ok(context) => context,
        Err(response) => return *response,
    };
    let Some(events) = state.services.streams.events.clone() else {
        return HttpError::not_yet_ported("event.subscribe").into_response();
    };
    let directory = context.directory.clone();
    let workspace = context.workspace_id.clone();
    let live = events
        .all()
        .filter_map(move |item| {
            let out = item.ok().filter(|event| {
                event.location.as_ref().is_some_and(|l| {
                    l.directory == directory
                        && (l.workspace_id.is_none() || l.workspace_id == workspace)
                })
            });
            futures_util::future::ready(out.map(|event| {
                json!({ "id": event.id, "type": event.event_type, "properties": event.data })
            }))
        })
        .boxed();
    let disposed = match state.services.streams.global.clone() {
        Some(bus) => {
            let directory = context.directory.clone();
            bus.subscribe()
                .filter_map(move |event| {
                    let matches = event.get("directory").and_then(Value::as_str) == Some(directory.as_str())
                        && event.pointer("/payload/type").and_then(Value::as_str) == Some(INSTANCE_DISPOSED_TYPE);
                    let out = matches.then(|| {
                        json!({
                            "id": event.pointer("/payload/id").cloned().unwrap_or_else(|| Value::String(event_id())),
                            "type": INSTANCE_DISPOSED_TYPE,
                            "properties": event.pointer("/payload/properties").cloned().unwrap_or_else(|| json!({})),
                        })
                    });
                    futures_util::future::ready(out)
                })
                .boxed()
        }
        None => stream::pending().boxed(),
    };
    // `stream.merge(disposed, halt left)` then `takeUntil(disposed)` (inclusive).
    let output = stream::select(
        live.map(Some).chain(stream::once(async { None })),
        disposed.map(Some),
    )
    .take_while(|item| futures_util::future::ready(item.is_some()))
    .filter_map(futures_util::future::ready)
    .scan(false, |done, event: Value| {
        if *done {
            return futures_util::future::ready(None);
        }
        *done = event.get("type").and_then(Value::as_str) == Some(INSTANCE_DISPOSED_TYPE);
        futures_util::future::ready(Some(sse::data(&event.to_string())))
    })
    .boxed();
    let heartbeat = sse::ticks(LEGACY_HEARTBEAT_INTERVAL, 1, || {
        sse::data(
            &json!({ "id": event_id(), "type": LEGACY_HEARTBEAT_TYPE, "properties": {} })
                .to_string(),
        )
    });
    let connected = json!({ "id": event_id(), "type": CONNECTED_TYPE, "properties": {} });
    let body = stream::once(async move { sse::data(&connected.to_string()) })
        .chain(sse::halt_left(output, heartbeat))
        .boxed();
    sse::response(body)
}

async fn global_events(State(state): State<AppState>) -> Response {
    let Some(bus) = state.services.streams.global.clone() else {
        return HttpError::not_yet_ported("global.event").into_response();
    };
    let events = bus
        .subscribe()
        .map(|event| sse::data(&event.to_string()))
        .boxed();
    let heartbeat = sse::ticks(LEGACY_HEARTBEAT_INTERVAL, 1, || {
        sse::data(
            &json!({ "payload": { "id": event_id(), "type": LEGACY_HEARTBEAT_TYPE, "properties": {} } }).to_string(),
        )
    });
    let connected =
        json!({ "payload": { "id": event_id(), "type": CONNECTED_TYPE, "properties": {} } });
    let body: BoxStream<'static, String> =
        stream::once(async move { sse::data(&connected.to_string()) })
            .chain(sse::halt_left(events, heartbeat))
            .boxed();
    sse::response(body)
}

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

async fn current_pty(
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

async fn legacy_pty(
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
    let context = match legacy_context(&state, routed).await {
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
    use super::*;

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
