//! Purpose: translate V1 HTTP operations to the composed legacy Session implementation.
//! Scope: request boundary.
//! Uses:
//! rust/crates/
//! |-- server/src/handlers/mod.rs
//! |-- server/src/middleware/errors.rs
//! `-- server/src/services/legacy.rs
//! External:
//! |-- axum::response::Response
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/mod.rs
//! Invariants: V1 replies have no current-API data envelope; domain errors retain legacy shape.
//! Contract tests: server/tests/http_legacy.rs, runtime/tests/process.rs

use crate::handlers::{Ctx, ok};
use crate::middleware::errors::HttpError;
use crate::services::legacy::LegacyError;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;

pub async fn handle(mut ctx: Ctx) -> Result<Response, HttpError> {
    if let Some(value) = ctx.query.get("roots").and_then(serde_json::Value::as_str) {
        let value = value == "true";
        ctx.query.insert("roots".into(), value.into());
    }
    let service = ctx
        .state
        .services
        .legacy_sessions
        .as_ref()
        .ok_or_else(|| HttpError::not_yet_ported(&ctx.endpoint.name))?;
    let context = ctx
        .legacy
        .as_ref()
        .ok_or_else(|| HttpError::Defect("missing legacy context".into()))?;
    match service
        .handle(
            &ctx.endpoint.name,
            context,
            &ctx.params,
            &ctx.query,
            ctx.payload,
        )
        .await
    {
        Ok(value) => Ok(ok(&value)),
        Err(LegacyError::NotFound(message)) => Ok(crate::middleware::errors::json(
            StatusCode::NOT_FOUND,
            &json!({"name":"NotFoundError","data":{"message":message}}),
        )),
        Err(LegacyError::BadRequest(_)) => Ok(crate::middleware::errors::json(
            StatusCode::BAD_REQUEST,
            &json!({"_tag":"BadRequest"}),
        )),
        Err(LegacyError::Unavailable(operation)) => Err(HttpError::not_yet_ported(&operation)),
        Err(LegacyError::Internal(message)) => Err(HttpError::Defect(message)),
    }
}
