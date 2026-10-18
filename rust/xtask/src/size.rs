//! Purpose: `cargo xtask size` task (owner: phase-01 child node, in progress).
//! Scope: build-time tooling.
//! Uses:
//! `-- none
//! External:
//! `-- anyhow::Result
//! Used by:
//! rust/xtask/
//! `-- src/main.rs
//! Invariants: deterministic, read-only unless `--fix` is passed.
//! Contract tests: xtask/tests/size.rs

use anyhow::{Result, bail};

/// Entry point for `cargo xtask size [args]`.
pub fn run(_args: &[String]) -> Result<()> {
    bail!("xtask size: not yet ported (phase-01 child node in progress)")
}
