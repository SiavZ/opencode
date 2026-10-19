//! Purpose: legacy Session HTTP service boundary preserving V1 JSON and domain errors.
//! Scope: process service, request context passed explicitly.
//! Uses:
//! rust/crates/
//! `-- server/src/middleware/legacy_routing.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- server/src/handlers/legacy.rs
//! `-- server/src/services/mod.rs
//! Invariants: implementations use real Session persistence and event projectors; no fabricated replies.
//! Contract tests: runtime/tests/process.rs

use crate::middleware::legacy_routing::RouteContext;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum LegacyError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("HTTP {status}: {body}")]
    Response { status: u16, body: Value },
    #[error("{0}")]
    Internal(String),
}

/// Explicit HTTP metadata for pagination, raw replies and no-content operations.
#[derive(Debug)]
pub struct LegacyResponse {
    pub body: Value,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl From<Value> for LegacyResponse {
    fn from(body: Value) -> Self {
        Self {
            body,
            status: 200,
            headers: Vec::new(),
        }
    }
}

/// Narrow V1 boundary. Query and payload are schema-validated by the HTTP host.
#[async_trait::async_trait]
pub trait LegacySessions: Send + Sync {
    async fn handle_response(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<LegacyResponse, LegacyError> {
        self.handle(operation, context, params, query, payload)
            .await
            .map(Into::into)
    }
    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError>;
}
