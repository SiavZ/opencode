//! Purpose: `cargo xtask check`, runs every Phase 01 gate (headers, edges, size, inventory,
//! fixtures) and reports all failures instead of stopping at the first.
//! Scope: build-time tooling.
//! Uses:
//! rust/xtask/
//! |-- src/edges.rs
//! |-- src/fixtures.rs
//! |-- src/headers.rs
//! |-- src/inventory.rs
//! `-- src/size.rs
//! External:
//! `-- anyhow::{Result, bail}
//! Used by:
//! rust/xtask/
//! `-- src/lib.rs
//! Invariants: read-only; exits non-zero when any gate fails.
//! Contract tests: xtask/tests/check.rs

use anyhow::{Result, bail};

type Gate = (&'static str, fn(&[String]) -> Result<()>);

const GATES: [Gate; 5] = [
    ("headers", crate::headers::run),
    ("edges", crate::edges::run),
    ("size", crate::size::run),
    ("inventory", crate::inventory::run),
    ("fixtures", crate::fixtures::run),
];

/// Entry point for `cargo xtask check`.
pub fn run(_args: &[String]) -> Result<()> {
    let failures = GATES
        .iter()
        .filter_map(|(name, gate)| {
            let result = gate(&[]);
            match &result {
                Ok(()) => println!("xtask check: {name} ok"),
                Err(error) => eprintln!("xtask check: {name} FAILED\n{error:#}"),
            }
            result.err().map(|_| *name)
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(());
    }
    bail!("xtask check failed: {}", failures.join(", "))
}
