//! Purpose: the production-gates.md evidence record (exact field set) emitted as one JSON line per
//! workflow and role, plus candidate identity hashing.
//! Scope: Phase 15 acceptance evidence serialization.
//! Uses:
//! rust/benchmarks/
//! `-- src/lib.rs
//! rust/tests/
//! `-- acceptance/src/workflow.rs
//! External:
//! |-- serde::Serialize
//! `-- sha2::{Digest, Sha256}
//! Used by:
//! rust/tests/
//! |-- acceptance/src/lib.rs
//! |-- acceptance/src/main.rs
//! |-- acceptance/src/runner.rs
//! `-- acceptance/tests/workflows.rs
//! Invariants: field names and order are exactly the 13 production-gates.md fields; `reviewer` is
//! never filled by the harness (an agent run is not review).
//! Contract tests: acceptance/tests/workflows.rs

use crate::workflow::Workflow;
use opencode_benchmarks::Target;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Baseline,
    Candidate,
}

impl Role {
    pub fn var(self) -> &'static str {
        match self {
            Role::Baseline => "BASELINE_CMD",
            Role::Candidate => "CANDIDATE_CMD",
        }
    }
}

/// Exactly the production-gates.md evidence record, in its documented order.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub requirement_id: String,
    pub baseline_source: String,
    pub owner: String,
    pub producer_consumer: String,
    pub workflow: String,
    pub fixture_live: String,
    pub target: String,
    pub candidate_hash: String,
    pub command_run_id: String,
    pub expected: Vec<String>,
    pub observed: Vec<String>,
    pub reviewer: String,
    pub gate_decision: String,
}

pub const FIELDS: [&str; 13] = [
    "requirement_id",
    "baseline_source",
    "owner",
    "producer_consumer",
    "workflow",
    "fixture_live",
    "target",
    "candidate_hash",
    "command_run_id",
    "expected",
    "observed",
    "reviewer",
    "gate_decision",
];

impl Evidence {
    pub fn new(w: &Workflow, role: Role, target: Option<&Target>) -> Self {
        let cmd = target.map_or_else(|| "<unset>".into(), |t| t.argv.join(" "));
        Self {
            requirement_id: w.requirement.clone(),
            baseline_source: w.baseline_source.clone(),
            owner: w.owner.clone(),
            producer_consumer: format!("{} -> {}", w.producer, w.consumer),
            workflow: format!(
                "{} ({})",
                w.id,
                if role == Role::Baseline {
                    "baseline"
                } else {
                    "candidate"
                }
            ),
            fixture_live: w.classification.clone(),
            target: platform(),
            candidate_hash: target.map_or_else(|| "none".into(), |t| cached_identity(role, t)),
            command_run_id: format!("{}={cmd} run={}", role.var(), run_id()),
            expected: w.expected(),
            observed: Vec::new(),
            reviewer: "unreviewed".into(),
            gate_decision: String::new(),
        }
    }

    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("evidence serializes")
    }
}

/// `<arch>-<os>` of the harness host, which is where the target runs (D13 matrix row).
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// One id per harness process, shared by all records of the run.
pub fn run_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        format!("{secs}-{}", std::process::id())
    })
}

/// Identity is resolved once per role per harness run, so concurrent commits by other agents
/// cannot give records of one run different hashes.
fn cached_identity(role: Role, t: &Target) -> String {
    static BASE: OnceLock<String> = OnceLock::new();
    static CAND: OnceLock<String> = OnceLock::new();
    let cell = if role == Role::Baseline { &BASE } else { &CAND };
    cell.get_or_init(|| identity(role, t)).clone()
}

/// `<ROLE>_HASH` override, else sha256 of an executable path, else `git-tree:<HEAD:cwd>` of the target cwd
/// (TypeScript dev command runs from source, so the commit is its identity).
pub fn identity(role: Role, t: &Target) -> String {
    if let Ok(h) = std::env::var(format!("{}_HASH", role.var().trim_end_matches("_CMD"))) {
        return h;
    }
    let exe = Path::new(&t.argv[0]);
    if exe.is_absolute()
        && let Ok(bytes) = std::fs::read(exe)
    {
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return format!("sha256:{hex}");
    }
    // The tree of the target cwd (e.g. packages/opencode) is stable across unrelated commits on
    // the shared branch, unlike HEAD. Uncommitted edits are not captured.
    let mut git = Command::new("git");
    git.args(["rev-parse", "HEAD:./"]);
    if let Some(cwd) = &t.cwd {
        git.current_dir(cwd);
    }
    match git.output() {
        Ok(o) if o.status.success() => {
            format!("git-tree:{}", String::from_utf8_lossy(&o.stdout).trim())
        }
        _ => "unknown".into(),
    }
}
