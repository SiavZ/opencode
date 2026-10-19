//! Purpose: crate root for Session runtime: durable admission and projection, process-global
//! execution coordination, the provider-turn runner, agents, lifecycle services and legacy
//! Session operations. Implements `opencode-session-api` traits.
//! Scope: native, tokio. Modules are owned by p04 child nodes (see contracts/phase-04/contract.md).
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/coordinator.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/question.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/src/summary.rs
//! |-- session-runtime/src/title.rs
//! `-- session-runtime/src/todo.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: no Session-ID-scoped service layers; admission never runs a provider.
//! Contract tests: session-runtime/tests/
// Module list is parent-owned (p04). Children add items only inside their own module files.

pub mod admission;
pub mod agents;
pub mod background;
pub mod children;
pub mod compaction;
pub mod coordinator;
pub mod execution;
pub mod inbox;
pub mod legacy;
pub mod projection;
pub mod question;
pub mod revert;
pub mod runner;
pub mod service;
pub mod store;
pub mod summary;
pub mod title;
pub mod todo;
