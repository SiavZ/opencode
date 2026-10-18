//! Purpose: endpoint-family translation from protocol operations to injected services. `handle`
//! picks the implementation by operation name; every operation without one answers the typed
//! 503 ServiceUnavailableError (NotYetPorted), never fake success.
//! Scope: per-request handlers.
//! Uses:
//! rust/crates/
//! |-- protocol/src/operation.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/cursor.rs
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/session.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! `-- server/src/services/mod.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- server/src/app.rs
//! `-- server/src/handlers/dispatch.rs
//! Invariants: handlers see already-decoded params/query/payload; location-scoped handlers read
//! the resolved `LocationInfo` from the context, never re-resolve it.
//! Contract tests: server/tests/http_errors.rs, server/tests/http_session.rs

pub mod cursor;
pub mod dispatch;
pub mod session;
pub mod streams;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use opencode_protocol::Endpoint;
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::middleware::errors::{HttpError, json as json_response};
use crate::middleware::legacy_routing::RouteContext;
use crate::services::LocationInfo;

/// Decoded request handed to an endpoint implementation.
pub struct Ctx {
    pub state: AppState,
    pub endpoint: &'static Endpoint,
    /// `/api/` route (tagged errors) vs legacy (NamedError bodies).
    pub current: bool,
    pub params: Map<String, Value>,
    pub query: Map<String, Value>,
    pub payload: Option<Value>,
    /// Resolved by the Location or SessionLocation middleware.
    pub location: Option<LocationInfo>,
    /// Resolved by legacy instance routing.
    pub legacy: Option<RouteContext>,
    pub raw_query: Option<String>,
}

impl Ctx {
    pub fn param(&self, name: &str) -> String {
        self.params
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub fn query_str(&self, name: &str) -> Option<String> {
        self.query
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub fn query_num(&self, name: &str) -> Option<f64> {
        self.query.get(name).and_then(Value::as_f64)
    }
}

/// Route a decoded request to its implementation.
pub async fn handle(ctx: Ctx) -> Result<Response, HttpError> {
    match ctx.endpoint.name.as_str() {
        "health.get" => Ok(ok(&json!({ "healthy": true }))),
        "location.get" => match &ctx.location {
            Some(location) => Ok(ok(location)),
            None => Err(HttpError::not_yet_ported("location")),
        },
        name if name.starts_with("session.") && ctx.current => session::handle(ctx).await,
        name => Err(HttpError::not_yet_ported(name)),
    }
}

/// 200 JSON.
pub fn ok<T: serde::Serialize + ?Sized>(value: &T) -> Response {
    json_response(StatusCode::OK, value)
}

/// 204 `HttpApiSchema.NoContent`.
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
