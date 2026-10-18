//! Purpose: CLI entrypoint for the platform matrix runner: print one JSON line per matrix row
//! (`--matrix FILE`, default the shipped matrix.toml). Exit 1 if any executed row fails.
//! Scope: Phase 15 platform runner skeleton.
//! Uses:
//! rust/tests/
//! `-- platform/src/lib.rs
//! External:
//! `-- anyhow::Result
//! Used by:
//! `-- none, entrypoint
//! Invariants: blocked rows never change the exit code; only executed failures do.
//! Contract tests: platform/tests/matrix.rs

use anyhow::{Context, Result};
use opencode_platform_runner::{Host, Matrix};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/matrix.toml"));
    while let Some(a) = args.next() {
        match a.as_str() {
            "--matrix" => path = args.next().context("--matrix needs a value")?.into(),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let host = Host::detect();
    let mut failed = false;
    for row in Matrix::load(&path)?.targets {
        let artifact = std::env::var_os(row.artifact_var()).map(PathBuf::from);
        let outcome = row.run(&host, artifact);
        failed |= outcome.decision == "fail";
        println!("{}", serde_json::to_string(&outcome)?);
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
