//! Purpose: Phase 01 tooling library behind `cargo xtask` (headers, edges, size, inventory,
//! fixtures, check) so integration tests drive the real task code against fixture trees.
//! Scope: build-time library, never a runtime dependency.
//! Uses:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/codegen/mod.rs
//! |-- src/edges.rs
//! |-- src/fixtures.rs
//! |-- src/headers.rs
//! |-- src/inventory.rs
//! |-- src/parity.rs
//! |-- src/size.rs
//! `-- src/workspace.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: every task takes an explicit workspace root (never the process cwd implicitly)
//! so tests can point it at temp fixture trees.
//! Contract tests: xtask/tests/

pub mod check;
pub mod codegen;
pub mod edges;
pub mod fixtures;
pub mod headers;
pub mod inventory;
pub mod parity;
pub mod size;
pub mod workspace;
