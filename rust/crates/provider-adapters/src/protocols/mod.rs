//! Purpose: protocol family roots. Each family directory is owned by one phase-03 leaf.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! `-- provider-adapters/src/protocols/openai/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: owned by the phase-03 parent; adding a family is a parent change.
//! Contract tests: provider-adapters/tests/ (per family)

pub mod anthropic;
pub mod bedrock;
pub mod gemini;
pub mod openai;
