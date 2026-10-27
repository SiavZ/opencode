//! Purpose: serialize public model listings and shared Zen CORS preflight responses.
//! Scope: credential-free catalog response serialization.
//! Uses:
//! `-- none
//! External:
//! |-- axum::{Json, response::Response}
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen.rs
//! Invariants: never return configured provider metadata or credentials.
//! Contract tests: hosted/src/console/zen.rs

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) fn models_response(ids: &[String]) -> Json<Value> {
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Json(
        json!({"object":"list","data":ids.iter().map(|id|json!({"id":id,"object":"model","created":created,"owned_by":"opencode"})).collect::<Vec<_>>()}),
    )
}

pub(super) async fn options_handler() -> Response {
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Authorization"),
    );
    response
}
