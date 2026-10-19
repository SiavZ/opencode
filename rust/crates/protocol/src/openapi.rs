//! Purpose: emit OpenAPI 3.1 documents from the operation registry, matching the baseline
//! Effect `OpenApi.fromApi` output (`packages/sdk/openapi.json`).
//! Scope: pure transformation registry -> serde_json::Value.
//! Uses:
//! rust/crates/
//! `-- protocol/src/operation.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- protocol/src/lib.rs
//! `-- server/src/app.rs
//! Invariants: path templates rewritten `:p` -> `{p}`, key order follows the baseline emitter.
//! Contract tests: protocol/tests/openapi.rs

use crate::operation::Api;

/// OpenAPI document for one or more APIs merged in order. Owner: p06.events-openapi.
pub fn document(apis: &[&Api]) -> serde_json::Value {
    let _ = apis;
    serde_json::json!({ "openapi": "3.1.0" })
}
