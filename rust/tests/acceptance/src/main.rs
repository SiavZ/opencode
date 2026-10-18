//! Purpose: CLI entrypoint: run every workflow in a directory against BASELINE_CMD and
//! CANDIDATE_CMD (each optional, `<VAR>_CWD` sets the working directory) and print evidence JSON
//! lines to stdout (or append to `--out <file>`). Exit 1 when any executed run fails.
//! Scope: Phase 15 acceptance harness binary.
//! Uses:
//! rust/benchmarks/
//! `-- src/lib.rs
//! rust/tests/
//! |-- acceptance/src/evidence.rs
//! |-- acceptance/src/runner.rs
//! `-- acceptance/src/workflow.rs
//! External:
//! `-- anyhow::Result
//! Used by:
//! `-- none, entrypoint
//! Invariants: an unset role yields `blocked` records, never a pass; ordering is workflow file
//! order, baseline before candidate.
//! Contract tests: acceptance/tests/workflows.rs
//!
//! Usage: acceptance [--workflows DIR] [--out FILE]

use anyhow::{Context, Result};
use opencode_acceptance::evidence::Role;
use opencode_acceptance::{runner, workflow::Workflow};
use opencode_benchmarks::Target;
use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/workflows"));
    let mut out: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--workflows" => dir = args.next().context("--workflows needs a value")?.into(),
            "--out" => out = Some(args.next().context("--out needs a value")?.into()),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let workflows = Workflow::load_dir(&dir)?;
    let mut sink: Box<dyn Write> = match &out {
        Some(p) => Box::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?,
        ),
        None => Box::new(std::io::stdout()),
    };
    let mut failed = false;
    for role in [Role::Baseline, Role::Candidate] {
        let target = Target::from_env(role.var());
        for w in &workflows {
            let ev = runner::run(w, role, target.as_ref());
            eprintln!("{:<40} {}", ev.workflow, ev.gate_decision);
            failed |= ev.gate_decision.starts_with("fail")
                || ev.gate_decision.starts_with("baseline-failed");
            writeln!(sink, "{}", ev.to_json_line())?;
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
