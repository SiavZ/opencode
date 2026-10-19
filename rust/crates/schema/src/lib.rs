//! Purpose: crate root for stable identifiers and shared serializable values used by every
//! layer, ported from `packages/schema/src` with byte-identical JSON. Wire DTOs belong to
//! `protocol`, domain services to their own crates.
//! Scope: pure values, no I/O except the OS random source used for identifiers.
//! Uses:
//! rust/crates/
//! |-- schema/src/agent.rs
//! |-- schema/src/event.rs
//! |-- schema/src/event_manifest.rs
//! |-- schema/src/filesystem.rs
//! |-- schema/src/identifier.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/integration.rs
//! |-- schema/src/location.rs
//! |-- schema/src/macros.rs
//! |-- schema/src/misc_event.rs
//! |-- schema/src/permission.rs
//! |-- schema/src/project.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/pty.rs
//! |-- schema/src/question.rs
//! |-- schema/src/reference.rs
//! |-- schema/src/revert.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_aux_event.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_input.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/v1.rs
//! |-- schema/src/values.rs
//! `-- schema/src/workspace.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- config/tests/markdown.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/project.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session_aux_event.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! |-- session-runtime/src/children/store.rs
//! |-- session-runtime/src/projection/adapter.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/tests/admission.rs
//! |-- storage-sqlite/src/repositories/credentials.rs
//! |-- storage-sqlite/src/repositories/domains.rs
//! |-- tools/src/search/core.rs
//! `-- tools/src/search/ripgrep.rs
//! Invariants: no dependency on any other first-party crate (see contracts/crate-edges.toml).
//! Module map (baseline file -> Rust module) lives in rust/contracts/phase-01/contract.md.
//! Contract tests: schema/tests/baseline_fixtures.rs

#[macro_use]
mod macros;

pub mod agent;
pub mod event;
pub mod event_manifest;
pub mod filesystem;
pub mod identifier;
pub mod ids;
pub mod integration;
pub mod location;
pub mod misc_event;
pub mod permission;
pub mod project;
pub mod prompt;
pub mod provider;
pub mod pty;
pub mod question;
pub mod reference;
pub mod revert;
pub mod session;
pub mod session_aux_event;
pub mod session_event;
pub mod session_input;
pub mod session_message;
pub mod v1;
pub mod values;
pub mod workspace;

pub use ids::*;
pub use values::{
    AbsolutePath, DateTimeUtc, Finite, Int, JsonObject, NonNegativeInt, PositiveInt, RelativePath,
};
