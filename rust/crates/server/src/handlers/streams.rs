//! Purpose: streaming endpoint handlers: current `/api/event`, durable `/api/session/:id/event`,
//! legacy `/event` and `/global/event`, and the PTY WebSocket connect routes (current
//! `/api/pty/:id/connect`, legacy `/pty/:id/connect`). Mounted by `app.rs` through `routes()`;
//! `owns()` tells the generic dispatcher to skip these operations.
//! Scope: per-connection handlers.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/sse.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/session_event.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/streams/pty.rs
//! |-- server/src/middleware/decode.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! |-- server/src/middleware/location.rs
//! `-- server/src/transport/sse.rs
//! External:
//! |-- axum::Router
//! |-- axum::extract::Path
//! |-- axum::extract::Request
//! |-- axum::extract::State
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- axum::routing::get
//! |-- decode::query
//! |-- futures_util::future::ready
//! |-- futures_util::stream::BoxStream
//! |-- futures_util::stream::StreamExt
//! |-- futures_util::stream::self
//! |-- legacy_routing::RouteContext
//! |-- legacy_routing::instance
//! |-- legacy_routing::route
//! |-- location::session
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_string
//! |-- sse::data
//! |-- sse::fallible_response
//! |-- sse::halt_left
//! |-- sse::halt_left_fallible
//! |-- sse::response
//! |-- sse::ticks
//! |-- str::to_string
//! |-- stream::once
//! |-- stream::pending
//! `-- stream::select
//! Used by:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/handlers/mod.rs
//! `-- server/src/handlers/streams/pty.rs
//! Invariants: current `/api/event` writes `server.connected` first, the 15 s `: heartbeat`
//! comment (first tick immediate, as in Effect `Stream.tick`), and ends the body abnormally when the
//! 256-slot subscriber overflows. Legacy streams write `{id,type,properties}` frames with a 10 s
//! `server.heartbeat` (first tick dropped). `/event` filters by instance directory/workspace and
//! ends after `server.instance.disposed`. Session events check placement (404 before streaming),
//! then replay durable events strictly after `after` and continue live. A stream failure is
//! written as the Effect `effect/httpapi/stream/failure` event. PTY connect checks existence (empty
//! 404) before ticket/origin (empty 403), then upgrades. Sockets are tracked by `SocketTracker`.
//! Contract tests: server/tests/sse_events.rs, server/tests/ws_pty.rs

mod pty;

pub use pty::{issue_ticket, parse_cursor};

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Path, Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, BoxStream, StreamExt};
use opencode_protocol::sse::{
    CONNECTED_TYPE, CURRENT_HEARTBEAT, CURRENT_HEARTBEAT_INTERVAL, CURRENT_SUBSCRIBER_CAPACITY,
    INSTANCE_DISPOSED_TYPE, LEGACY_HEARTBEAT_INTERVAL, LEGACY_HEARTBEAT_TYPE,
};
use opencode_schema::ids::EventID;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::middleware::errors::HttpError;
use crate::middleware::legacy_routing::{self, Plan};
use crate::middleware::{decode, location};
use crate::transport::sse;

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
        .route("/api/pty/{ptyID}/connect", get(pty::current_pty))
        .route("/event", get(legacy_events))
        .route("/global/event", get(global_events))
        .route("/pty/{ptyID}/connect", get(pty::legacy_pty))
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
    let (mut parts, body) = request.into_parts();
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
        Ok((Plan::WebSocket(target), _)) => {
            let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &()).await;
            Err(Box::new(
                crate::transport::proxy_socket::upgrade(state, target, &parts.headers, upgrade)
                    .await,
            ))
        }
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
