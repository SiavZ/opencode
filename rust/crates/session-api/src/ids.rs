//! Purpose: re-export the Session-relevant identifier and reference values owned by
//! `opencode-schema` so Session consumers have one import point. Defines no new types.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/location.rs
//! |-- schema/src/provider.rs
//! `-- schema/src/values.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/mod.rs
//! rust/crates/
//! |-- server/src/handlers/session.rs
//! |-- server/tests/support/mod.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/agents.rs
//! `-- session-runtime/src/service.rs
//! Invariants: schema owns the brands; `ses` IDs descending, `msg_` IDs ascending.
//! Contract tests: session-api/tests/wire.rs

pub use opencode_schema::ids::{
    AgentID, MessageID, ModelID, ProjectID, ProviderID, SessionID, WorkspaceID,
};
pub use opencode_schema::location::Ref as LocationRef;
pub use opencode_schema::provider::ModelRef;
pub use opencode_schema::values::{
    AbsolutePath, DateTimeUtc, Finite, NonNegativeInt, RelativePath,
};

/// `AgentV2.defaultID`.
pub fn default_agent_id() -> AgentID {
    AgentID::new("build")
}

/// `switchModel` short-circuit: same provider, model and variant (missing variant is `"default"`).
pub fn same_model_selection(left: &ModelRef, right: &ModelRef) -> bool {
    left.provider_id == right.provider_id
        && left.id == right.id
        && left.variant.as_ref().map_or("default", |v| v.as_str())
            == right.variant.as_ref().map_or("default", |v| v.as_str())
}
