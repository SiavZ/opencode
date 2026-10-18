//! Purpose: crate root for permission policy: wildcard matching, V2 rule evaluation, the
//! ask/reply/deny lifecycle with saved grants, plus legacy V1 compatibility evaluation.
//! Scope: pure policy plus an in-memory pending registry over injected ports.
//! Uses:
//! rust/crates/
//! |-- permissions/src/rule.rs
//! |-- permissions/src/service.rs
//! `-- permissions/src/wildcard.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-05, consumers tool-api, tools, session-runtime, server, integration-adapters)
//! Invariants: one policy implementation. Server/UI/ACP reply through `PermissionService::reply`.
//! Contract tests: permissions/tests/service.rs

pub mod rule;
pub mod service;
pub mod wildcard;

pub use rule::{Effect, Rule, Ruleset, evaluate, merge, wholly_disabled};
pub use service::{
    AskResult, AssertInput, PermissionError, PermissionEvent, PermissionEvents, PermissionRequest,
    PermissionService, PolicySource, Reply, ReplyInput, SavedGrant, SavedGrants, Source,
};
