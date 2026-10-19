//! Purpose: crate root for system context (`packages/core/src/system-context`) plus Session-owned
//! history selection and context epoch persistence (`packages/core/src/session/{history,context-epoch}.ts`).
//! Scope: native. Domain producers (skills, references, instructions) live with their domains.
//! Uses:
//! rust/crates/
//! |-- context/src/builtins.rs
//! |-- context/src/epoch.rs
//! |-- context/src/history.rs
//! |-- context/src/registry.rs
//! `-- context/src/source/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: unavailable source is not removal; duplicate keys are rejected.
//! Contract tests: context/tests/
// Module list is parent-owned (p04).

pub mod builtins;
pub mod epoch;
pub mod history;
pub mod registry;
pub mod source;
