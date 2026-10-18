//! Purpose: Rust-owned workspace tooling entrypoint (`cargo xtask <task>`). Phase 01 owns
//! dispatch; other phases add only their `mod` and one dispatch arm.
//! Scope: build-time process, never a runtime dependency.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/mod.rs
//! |-- src/distribution/mod.rs
//! `-- src/lib.rs
//! External:
//! `-- anyhow::{bail, Result}
//! Used by:
//! `-- none, entrypoint
//! Invariants: tasks are deterministic and never mutate files outside `rust/` unless asked.
//! Contract tests: xtask/tests/

mod cloud;
mod distribution;

use anyhow::{Result, bail};

const HELP: &str = "cargo xtask <task> [args]

Phase 01:
  headers [--fix]   check (or regenerate) Uses/Used by header trees
  edges             resolved native + wasm32 crate graph vs contracts/crate-edges.toml
  size              500-line rule vs contracts/modularity-reviews.toml
  inventory         TS baseline registries vs coverage IDs
  fixtures [run]    validate versioned fixtures, run differential comparisons
  check             all of the above
Phase 13:
  cloud             infrastructure plans";

fn main() -> Result<()> {
    let args = std::env::args().skip(2).collect::<Vec<_>>();
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "cloud" => cloud::run(&args),
        "distribution" => distribution::run(&args),
        "headers" => xtask::headers::run(&args),
        "edges" => xtask::edges::run(&args),
        "size" => xtask::size::run(&args),
        "inventory" => xtask::inventory::run(&args),
        "fixtures" => xtask::fixtures::run(&args),
        "check" => xtask::check::run(&args),
        "" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        other => bail!("unknown xtask task: {other}"),
    }
}
