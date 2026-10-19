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
//! |-- anyhow::Result
//! `-- anyhow::bail
//! Used by:
//! rust/xtask/
//! |-- src/lib.rs
//! |-- src/main.rs
//! `-- tests/gates.rs
//! Invariants: read-only; exits non-zero when any gate fails.
//! Contract tests: xtask/tests/check.rs

use anyhow::{Result, bail};

pub type Gate = (&'static str, fn(&[String]) -> Result<()>);

const GATES: [Gate; 5] = [
    ("headers", crate::headers::run),
    ("edges", crate::edges::run),
    ("size", crate::size::run),
    ("inventory", crate::inventory::run),
    ("fixtures", |args| {
        let mut fixture_args = vec![String::from("run")];
        fixture_args.extend_from_slice(args);
        crate::fixtures::run(&fixture_args)
    }),
];

/// Entry point for `cargo xtask check`.
pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() && args != ["--strict"] {
        bail!("usage: check [--strict]; no gate may be skipped");
    }
    evaluate_args(&GATES, args)
}

/// Execute every gate even after a failure, preserving each diagnostic.
pub fn evaluate(gates: &[Gate]) -> Result<()> {
    evaluate_args(gates, &[])
}

fn evaluate_args(gates: &[Gate], args: &[String]) -> Result<()> {
    let failures = gates
        .iter()
        .filter_map(|(name, gate)| {
            let result = gate(if *name == "fixtures" { args } else { &[] });
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
