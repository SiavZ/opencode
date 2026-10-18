//! Purpose: contract tests for the acceptance harness: every shipped workflow parses, the
//! evidence record has exactly the production-gates.md fields in order, unset roles are blocked
//! (never pass), JSON matching/templating behave, and a real stub process exercises pass/fail.
//! Scope: fixture tests; the live TS baseline run is the `acceptance` binary (see
//! rust/contracts/phase-15/acceptance.md).
//! Uses:
//! rust/benchmarks/
//! `-- src/lib.rs
//! rust/tests/
//! |-- acceptance/src/evidence.rs
//! |-- acceptance/src/runner.rs
//! `-- acceptance/src/workflow.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: no test reads real user data; stub targets are /bin/sh one-liners.
//! Contract tests: this file

use opencode_acceptance::evidence::{FIELDS, Role};
use opencode_acceptance::runner::{self, check_json, template};
use opencode_acceptance::workflow::Workflow;
use opencode_benchmarks::Target;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

fn shipped() -> Vec<Workflow> {
    Workflow::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("workflows")).expect("load")
}

fn cli_workflow(regex: &str) -> Workflow {
    toml::from_str(&format!(
        r#"
id = "stub"
requirement = "TEST-00"
baseline_source = "none"
owner = "phase-15"
producer = "p"
consumer = "c"
classification = "fixture"
[[step]]
kind = "cli"
args = ["--version"]
expect_exit = 0
expect_stdout_regex = '{regex}'
"#
    ))
    .expect("parse")
}

#[test]
fn shipped_workflows_parse_with_coverage_ids() {
    let w = shipped();
    assert_eq!(
        w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["cli-version", "serve-health", "session-create-list"]
    );
    assert!(w.iter().all(|w| w.requirement.starts_with("PKG-")));
}

#[test]
fn unknown_fields_are_rejected() {
    let err = toml::from_str::<Workflow>(
        "id='x'\nrequirement='r'\nbaseline_source='b'\nowner='o'\nproducer='p'\nconsumer='c'\nclassification='live'\n[[step]]\nkind='cli'\nargs=[]\nexpect_exitt=0\n",
    );
    assert!(err.is_err());
}

#[test]
fn evidence_has_exact_gate_fields_and_unset_is_blocked() {
    let w = &shipped()[0];
    let ev = runner::run(w, Role::Candidate, None);
    let v = serde_json::to_value(&ev).expect("json");
    let keys: Vec<_> = v.as_object().expect("obj").keys().cloned().collect();
    assert_eq!(keys, FIELDS);
    assert!(
        ev.gate_decision.starts_with("blocked:"),
        "{}",
        ev.gate_decision
    );
    assert_eq!(ev.reviewer, "unreviewed");
    assert_eq!(ev.candidate_hash, "none");
}

#[test]
fn stub_process_passes_and_fails_on_real_output() {
    let t = Target::parse("/bin/sh -c 'echo 1.2.3' sh", None).expect("target");
    let pass = runner::run(&cli_workflow(r"^\d+\.\d+\.\d+$"), Role::Candidate, Some(&t));
    assert_eq!(
        pass.gate_decision, "pass (pending review)",
        "{:?}",
        pass.observed
    );
    assert!(pass.candidate_hash.starts_with("sha256:"));
    let fail = runner::run(&cli_workflow("^local$"), Role::Candidate, Some(&t));
    assert_eq!(fail.gate_decision, "fail");
    assert!(fail.observed.last().expect("log").contains("FAILED"));
    let base = runner::run(&cli_workflow("^local$"), Role::Baseline, Some(&t));
    assert!(base.gate_decision.starts_with("baseline-failed"));
}

#[test]
fn json_pointer_matching_and_templating() {
    let vars = HashMap::from([("id".to_string(), "ses_1".to_string())]);
    assert_eq!(
        template("/session/${id}?x=${nope}", &vars),
        "/session/ses_1?x=${nope}"
    );
    let body = json!({"id": "ses_1", "tokens": {"input": 0}});
    let ok = json!({"/id": "${id}", "/tokens/input": 0});
    assert!(check_json(&body, ok.as_object().expect("obj"), &vars).is_ok());
    let bad = json!({"/tokens/input": 1});
    assert!(check_json(&body, bad.as_object().expect("obj"), &vars).is_err());
    let missing = json!({"/nope": null});
    assert!(check_json(&body, missing.as_object().expect("obj"), &vars).is_err());
}
