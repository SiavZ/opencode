//! Purpose: legacy PublicApi registry (`packages/opencode/src/server/routes/instance/httpapi/api.ts`:
//! root control/control-plane/global, event, instance groups, PTY connect) kept separate from the
//! current API because envelopes, pagination and errors differ.
//! Scope: static registry construction, built once per process.
//! Uses:
//! rust/crates/
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/lib.rs
//! Invariants: groups marked `legacy = true`, registration order matches `OpenCodeHttpApi`.
//! Contract tests: protocol/tests/registry.rs

use crate::operation::Api;
use std::sync::OnceLock;

/// The legacy PublicApi families. Populated by the legacy registry module (p06.legacy-registry).
pub fn api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(|| Api {
        identifier: "opencode".to_string(),
        title: "opencode".to_string(),
        version: "1.0.0".to_string(),
        description: Some("opencode api".to_string()),
        groups: Vec::new(),
        definitions: Vec::new(),
    })
}
