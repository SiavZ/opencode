//! Purpose: per-endpoint request pipeline shared by every mounted protocol operation: legacy
//! workspace routing, location/session-location middleware, params/query/payload decoding with
//! schema rejections, then the endpoint handler (or a typed NotYetPorted answer).
//! Scope: per-request.
//! Uses:
//! rust/crates/
//! |-- protocol/src/lib.rs
//! |-- protocol/src/operation.rs
//! |-- server/src/app.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/middleware/decode.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! `-- server/src/middleware/location.rs
//! External:
//! |-- axum::body::Body
//! |-- axum::body::to_bytes
//! |-- axum::extract::Path
//! |-- axum::extract::Request
//! |-- axum::http::StatusCode
//! |-- axum::http::header
//! |-- axum::response::IntoResponse
//! |-- axum::response::Response
//! |-- decode::params
//! |-- decode::payload
//! |-- decode::query
//! |-- header::CONTENT_TYPE
//! |-- legacy_routing::instance
//! |-- legacy_routing::route
//! |-- location::reference
//! |-- location::resolve
//! |-- location::session
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/handlers/mod.rs
//! `-- server/src/handlers/session.rs
//! Invariants: middleware runs before decoding (Effect `applyMiddleware` wraps the decoder), decode
//! order is Params, Query, Payload. Current (`/api/`) rejections use the tagged
//! InvalidRequestError, legacy ones the NamedError BadRequest body. A JSON payload with another
//! media type answers 415 `Unsupported content-type: <type>`.
//! Contract tests: server/tests/http_errors.rs, server/tests/http_session.rs

use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use opencode_protocol::operation::{Middleware, PayloadEncoding};
use opencode_protocol::{Api, Endpoint};
use serde_json::{Map, Value};

use crate::app::AppState;
use crate::handlers::{Ctx, handle};
use crate::middleware::errors::{HttpError, text};
use crate::middleware::legacy_routing::{self, Plan};
use crate::middleware::{decode, location};

/// Request bodies above this size are rejected by the decoder (matches no baseline limit; keeps
/// memory bounded).
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Run one request for `endpoint`.
pub async fn run(
    state: AppState,
    api: &'static Api,
    endpoint: &'static Endpoint,
    raw_params: Vec<(String, String)>,
    request: Request,
) -> Response {
    match pipeline(state, api, endpoint, raw_params, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn pipeline(
    state: AppState,
    api: &'static Api,
    endpoint: &'static Endpoint,
    raw_params: Vec<(String, String)>,
    request: Request,
) -> Result<Response, HttpError> {
    let current = endpoint.path.starts_with("/api/");
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().map(str::to_string);

    let (legacy, body) = if endpoint.middleware.contains(&Middleware::LegacyInstance) {
        let (plan, body) = legacy_routing::route(
            &state.services,
            &state.client,
            &parts.method,
            &path,
            query.as_deref(),
            &parts.headers,
            body,
        )
        .await?;
        match plan {
            Plan::Answered(response) => return Ok(response),
            Plan::Local(context) => (
                Some(legacy_routing::instance(context).await?),
                body.unwrap_or_else(Body::empty),
            ),
        }
    } else {
        (None, body)
    };

    let location = if endpoint.middleware.contains(&Middleware::SessionLocation) {
        Some(location::session(&state.services, &raw_params).await?)
    } else if endpoint.middleware.contains(&Middleware::Location) {
        let reference = location::reference(query.as_deref(), &parts.headers)?;
        Some(location::resolve(&state.services, &reference).await?)
    } else {
        None
    };

    let params = decode::params(api, &endpoint.params, &raw_params)
        .map_err(|reason| HttpError::schema(current, "Params", &reason))?;
    let decoded_query = decode::query(api, &endpoint.query, query.as_deref())
        .map_err(|reason| HttpError::schema(current, "Query", &reason))?;

    let payload = match &endpoint.payload {
        None => None,
        Some(schema) => {
            if endpoint.payload_encoding != PayloadEncoding::Json {
                return Err(HttpError::not_yet_ported(&format!(
                    "{} ({:?} payload)",
                    endpoint.name, endpoint.payload_encoding
                )));
            }
            let media = parts
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase()
                })
                .unwrap_or_default();
            let legacy_create = !current && endpoint.name == "session.create";
            if media != "application/json" && !legacy_create {
                return Ok(text(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "text/plain",
                    format!("Unsupported content-type: {media}"),
                ));
            }
            let bytes = to_bytes(body, MAX_BODY)
                .await
                .map_err(|e| HttpError::Defect(e.to_string()))?;
            // Effect: empty text decodes as `undefined`, a malformed body is a defect.
            let value = if bytes.is_empty() || (legacy_create && legacy_empty_body(&bytes)) {
                None
            } else {
                Some(serde_json::from_slice::<Value>(&bytes).map_err(|e| {
                    if legacy_create {
                        HttpError::LegacyRawBadRequest
                    } else {
                        HttpError::Defect(format!("invalid JSON body: {e}"))
                    }
                })?)
            };
            match value {
                Some(value) => {
                    decode::payload(api, schema, &value).map_err(|reason| {
                        if legacy_create {
                            HttpError::LegacyRawBadRequest
                        } else {
                            HttpError::schema(current, "Payload", &reason)
                        }
                    })?;
                    Some(value)
                }
                None if endpoint.payload_required => {
                    return Err(HttpError::schema(
                        current,
                        "Payload",
                        "Expected object, got undefined",
                    ));
                }
                None => None,
            }
        }
    };

    let ctx = Ctx {
        state,
        endpoint,
        current,
        params,
        query: decoded_query,
        payload,
        location,
        legacy,
        raw_query: query,
    };
    handle(ctx).await
}

/// JavaScript String.trim also removes BOM and NBSP, but not Unicode NEL.
fn legacy_empty_body(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        text.chars().all(|character| {
            matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' |
            '\u{3000}' | '\u{feff}')
        })
    })
}

/// Collect axum path params in template order.
pub fn path_params(params: Option<Path<Vec<(String, String)>>>) -> Vec<(String, String)> {
    params.map(|Path(p)| p).unwrap_or_default()
}

/// Helper used by handlers to decode a validated JSON object into a typed value. Failures are
/// schema rejections of `kind`.
pub fn typed<T: serde::de::DeserializeOwned>(
    current: bool,
    kind: &str,
    value: Value,
) -> Result<T, HttpError> {
    serde_json::from_value(value).map_err(|e| HttpError::schema(current, kind, &e.to_string()))
}

/// `Map` to `Value` convenience.
pub fn object(map: &Map<String, Value>) -> Value {
    Value::Object(map.clone())
}
