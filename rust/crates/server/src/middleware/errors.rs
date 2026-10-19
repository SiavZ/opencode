//! Purpose: error envelopes on the HTTP boundary: current tagged errors, legacy NamedError
//! bodies, schema rejections, empty 401s and the defect path with an `err_` reference.
//! Scope: per-response value conversion.
//! Uses:
//! rust/crates/
//! `-- protocol/src/errors.rs
//! External:
//! |-- axum::response::{IntoResponse, Response}
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/dispatch.rs
//! |-- server/src/handlers/session.rs
//! `-- server/src/middleware/auth.rs
//! Invariants: current routes answer `{"_tag":..}` with the declared status. Legacy schema
//! rejections answer `{"name":"BadRequest","data":{message,kind}}`. Defects answer 500
//! `{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details.","ref":"err_xxxxxxxx"}}`
//! (packages/opencode/src/server/routes/instance/httpapi/middleware/error.ts). Unported
//! capabilities answer 503 ServiceUnavailableError, never success.
//! Contract tests: server/tests/http_errors.rs

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opencode_protocol::errors::{ApiError, WWW_AUTHENTICATE};

/// Message used by every defect response.
pub const DEFECT_MESSAGE: &str = "Unexpected server error. Check server logs for details.";

/// Truncation limit shared by both schema-error middlewares.
const REASON_LIMIT: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// Declared current-API error.
    #[error("{0}")]
    Api(Box<ApiError>),
    #[error("legacy raw bad request")]
    LegacyRawBadRequest,
    /// Legacy schema rejection (`packages/opencode/.../middleware/schema-error.ts`).
    #[error("legacy bad request: {message}")]
    LegacyBadRequest { message: String, kind: String },
    /// Legacy `HttpApiError.Unauthorized` or router-level rejection: empty body.
    #[error("unauthorized")]
    UnauthorizedEmpty,
    /// Capability not composed or not ported in this host. 503 ServiceUnavailableError.
    #[error("not yet ported: {operation}")]
    NotYetPorted { operation: String },
    /// Unexpected failure. Logged with a reference, 500 NamedError Unknown.
    #[error("defect: {0}")]
    Defect(String),
}

impl From<ApiError> for HttpError {
    fn from(error: ApiError) -> Self {
        HttpError::Api(Box::new(error))
    }
}

impl HttpError {
    pub fn not_yet_ported(operation: &str) -> Self {
        HttpError::NotYetPorted {
            operation: operation.to_string(),
        }
    }

    /// Schema rejection for a route: current (`/api/`) routes use the tagged envelope with an
    /// ASCII `...` suffix, legacy routes the NamedError body with a `…` suffix.
    pub fn schema(current: bool, kind: &str, reason: &str) -> Self {
        if current {
            return ApiError::schema_rejection(reason, kind).into();
        }
        let units: Vec<u16> = reason.encode_utf16().collect();
        let message = if units.len() <= REASON_LIMIT {
            reason.to_string()
        } else {
            format!(
                "{}\u{2026} ({} more chars)",
                String::from_utf16_lossy(&units[..REASON_LIMIT]),
                units.len() - REASON_LIMIT
            )
        };
        HttpError::LegacyBadRequest {
            message,
            kind: kind.to_string(),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        match self {
            HttpError::LegacyRawBadRequest => json(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"_tag":"BadRequest"}),
            ),
            HttpError::Api(error) => {
                let status = StatusCode::from_u16(error.status())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let unauthorized = matches!(*error, ApiError::UnauthorizedError { .. });
                let mut response = json(status, &*error);
                if unauthorized {
                    response.headers_mut().insert(
                        header::WWW_AUTHENTICATE,
                        HeaderValue::from_static(WWW_AUTHENTICATE),
                    );
                }
                response
            }
            HttpError::LegacyBadRequest { message, kind } => json(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "name": "BadRequest", "data": { "message": message, "kind": kind } }),
            ),
            HttpError::UnauthorizedEmpty => {
                let mut response = StatusCode::UNAUTHORIZED.into_response();
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static(WWW_AUTHENTICATE),
                );
                response
            }
            HttpError::NotYetPorted { operation } => {
                tracing::warn!(operation, "operation not ported in this host");
                let error = ApiError::ServiceUnavailableError {
                    message: format!("Operation {operation} is not available in this host yet"),
                    service: Some(operation),
                };
                json(StatusCode::SERVICE_UNAVAILABLE, &error)
            }
            HttpError::Defect(cause) => {
                let reference = defect_ref();
                tracing::error!(reference, cause, "failed");
                json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &serde_json::json!({
                        "name": "UnknownError",
                        "data": { "message": DEFECT_MESSAGE, "ref": reference },
                    }),
                )
            }
        }
    }
}

/// `err_` plus the first 8 characters of a random UUID, like the baseline.
pub fn defect_ref() -> String {
    format!("err_{}", &uuid::Uuid::new_v4().to_string()[..8])
}

/// JSON response with `content-type: application/json`.
pub fn json<T: serde::Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (status, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(error) => HttpError::Defect(error.to_string()).into_response(),
    }
}

/// Plain text response with an explicit content type (routing failures).
pub fn text(status: StatusCode, content_type: &str, body: String) -> Response {
    let mut response = (status, body).into_response();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}
