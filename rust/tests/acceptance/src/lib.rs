//! Purpose: library surface of the Phase 15 black-box acceptance harness (workflow model, runner,
//! evidence records).
//! Scope: Phase 15 acceptance harness.
//! Uses:
//! rust/tests/
//! |-- acceptance/src/evidence.rs
//! |-- acceptance/src/runner.rs
//! `-- acceptance/src/workflow.rs
//! External:
//! `-- none
//! Used by:
//! rust/tests/
//! |-- acceptance/src/main.rs
//! `-- acceptance/tests/workflows.rs
//! Invariants: no product crates are linked; targets are only ever driven as processes.
//! Contract tests: acceptance/tests/workflows.rs

pub mod evidence;
pub mod runner;
pub mod workflow;
