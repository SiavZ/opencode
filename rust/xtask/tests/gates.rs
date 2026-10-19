//! Purpose: regression tests for the real Phase 01 gates, including negative cases.
//! Scope: isolated fixture workspaces plus read-only checked-in metadata.
//! Uses:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/edges.rs
//! |-- src/fixtures.rs
//! |-- src/headers.rs
//! |-- src/inventory.rs
//! |-- src/size.rs
//! |-- src/workspace.rs
//! `-- tests/gates/headers.rs
//! External:
//! |-- anyhow::bail
//! |-- check::Gate
//! |-- check::evaluate
//! |-- edges::Contract
//! |-- edges::metadata
//! |-- edges::validate
//! |-- fixtures::validate
//! |-- headers::validate
//! |-- inventory::Entry
//! |-- inventory::compare
//! |-- inventory::discover
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- size::validate
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- toml::from_str
//! Used by:
//! rust/xtask/
//! `-- tests/gates/headers.rs
//! Invariants: tests never bless real contracts or mutate process-global environment.
//! Contract tests: this file

#[path = "gates/headers.rs"]
mod header_cases;

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use xtask::{check, edges, fixtures, headers, inventory, size};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn tree() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for path in ["crates", "bins", "xtask", "contracts"] {
        std::fs::create_dir(temp.path().join(path)).unwrap();
    }
    write(temp.path(), "contracts/runtime-edges.toml", "");
    write(temp.path(), "contracts/modularity-reviews.toml", "");
    temp
}

fn contract() -> edges::Contract {
    toml::from_str(
        r#"
[crates.a]
owner = ["01"]
target = "portable"
allow = ["b"]
[crates.b]
owner = ["02"]
target = "native"
allow = []
[bins]
[xtask]
owner = ["01"]
target = "native"
allow = []
"#,
    )
    .unwrap()
}

fn graph() -> Value {
    json!({
        "workspace_members": ["a", "b", "xtask"],
        "packages": [
            {"id":"a", "manifest_path":"/fixture/crates/a/Cargo.toml"},
            {"id":"b", "manifest_path":"/fixture/crates/b/Cargo.toml"},
            {"id":"xtask", "manifest_path":"/fixture/xtask/Cargo.toml"}
        ],
        "resolve": {"nodes": [
            {"id":"a", "deps":[]}, {"id":"b", "deps":[]}, {"id":"xtask", "deps":[]}
        ]}
    })
}

#[test]
fn edges_accepts_declared_acyclic_graph() {
    assert!(
        edges::validate(Path::new("/fixture"), &contract(), &graph())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn edges_rejects_renamed_build_dependency_and_native_transitive_closure() {
    let mut graph = graph();
    graph["resolve"]["nodes"][0]["deps"] =
        json!([{"name":"renamed", "pkg":"b", "dep_kinds":[{"kind":"build"}]}]);
    let mut rules = contract();
    rules.crates.get_mut("a").unwrap().allow.clear();
    let errors = edges::validate(Path::new("/fixture"), &rules, &graph).unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("unapproved normal/build edge crates/a -> crates/b")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("closure crates/a resolves native-only crates/b")),
        "{errors:?}"
    );
}

#[test]
fn dev_edges_are_exempt_from_allowlists_but_cycles_are_not() {
    let mut graph = graph();
    graph["resolve"]["nodes"][0]["deps"] = json!([{"pkg":"b", "dep_kinds":[{"kind":"dev"}]}]);
    let mut rules = contract();
    rules.crates.get_mut("a").unwrap().allow.clear();
    assert!(
        edges::validate(Path::new("/fixture"), &rules, &graph)
            .unwrap()
            .is_empty()
    );
    graph["resolve"]["nodes"][1]["deps"] = json!([{"pkg":"a", "dep_kinds":[{"kind":"dev"}]}]);
    let errors = edges::validate(Path::new("/fixture"), &rules, &graph).unwrap();
    assert_eq!(
        errors
            .iter()
            .filter(|e| e.contains("dependency cycle"))
            .count(),
        2
    );
}

#[test]
fn edges_rejects_undeclared_and_stale_members() {
    let mut graph = graph();
    graph["packages"][0]["manifest_path"] = json!("/fixture/crates/surprise/Cargo.toml");
    let errors = edges::validate(Path::new("/fixture"), &contract(), &graph).unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("undeclared workspace member crates/surprise"))
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("stale declaration crates/a"))
    );
}

#[test]
fn cargo_resolution_preserves_target_filters_and_renames() {
    let temp = tree();
    write(
        temp.path(),
        "Cargo.toml",
        "[workspace]\nresolver='3'\nmembers=['crates/*','xtask']\n",
    );
    write(
        temp.path(),
        "crates/a/Cargo.toml",
        "[package]\nname='a'\nversion='0.0.0'\nedition='2024'\n[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]\nalias={package='b',path='../b'}\n",
    );
    write(
        temp.path(),
        "crates/b/Cargo.toml",
        "[package]\nname='b'\nversion='0.0.0'\nedition='2024'\n",
    );
    write(
        temp.path(),
        "xtask/Cargo.toml",
        "[package]\nname='xtask'\nversion='0.0.0'\nedition='2024'\n",
    );
    for source in [
        "crates/a/src/lib.rs",
        "crates/b/src/lib.rs",
        "xtask/src/lib.rs",
    ] {
        write(temp.path(), source, "");
    }
    assert!(
        std::process::Command::new("cargo")
            .current_dir(temp.path())
            .args(["generate-lockfile", "--offline"])
            .status()
            .unwrap()
            .success()
    );
    let mut rules = contract();
    rules.crates.get_mut("a").unwrap().allow.clear();
    let wasm = edges::metadata(temp.path(), Some("wasm32-unknown-unknown")).unwrap();
    assert!(
        edges::validate(temp.path(), &rules, &wasm)
            .unwrap()
            .is_empty()
    );
    let host = edges::metadata(temp.path(), Some("aarch64-apple-darwin")).unwrap();
    assert!(
        edges::validate(temp.path(), &rules, &host)
            .unwrap()
            .iter()
            .any(|e| e.contains("unapproved normal/build edge"))
    );
}

const HEADER: &str = "//! Purpose: test source.\n//! Scope: fixture.\n//! Uses:\n//! `-- none\n//! External:\n//! `-- none\n//! Used by:\n//! `-- none, entrypoint\n//! Invariants: preserve author text.\n//! Contract tests: this file\n\n";

#[test]
fn header_fix_resolves_imports_modules_and_inverse_runtime_edges_idempotently() {
    let temp = tree();
    write(
        temp.path(),
        "crates/a/src/lib.rs",
        &format!(
            "{HEADER}mod child;\nuse crate::child::Thing;\nuse serde::{{Serialize, Deserialize}};\n"
        ),
    );
    write(
        temp.path(),
        "crates/a/src/child.rs",
        &format!("{HEADER}pub struct Thing;\n"),
    );
    write(
        temp.path(),
        "crates/b/src/lib.rs",
        &format!("{HEADER}pub struct Other;\n"),
    );
    write(
        temp.path(),
        "contracts/runtime-edges.toml",
        "[[edge]]\nfrom='rust/crates/b/src/lib.rs'\nto='rust/crates/a/src/child.rs'\nkind='dynamic-dispatch'\nreason='fixture registration'\n",
    );
    assert!(!headers::validate(temp.path(), false).unwrap().is_empty());
    assert!(headers::validate(temp.path(), true).unwrap().is_empty());
    assert!(headers::validate(temp.path(), false).unwrap().is_empty());
    let source = std::fs::read_to_string(temp.path().join("crates/a/src/lib.rs")).unwrap();
    assert!(source.contains("`-- a/src/child.rs"));
    assert!(source.contains("serde::Deserialize"));
    assert!(source.contains("//! Invariants: preserve author text."));
    let child = std::fs::read_to_string(temp.path().join("crates/a/src/child.rs")).unwrap();
    assert!(child.contains("|-- a/src/lib.rs"));
    assert!(child.contains("`-- b/src/lib.rs"));
}

#[test]
fn headers_do_not_invent_missing_author_fields() {
    let temp = tree();
    write(temp.path(), "crates/a/src/lib.rs", "pub struct Missing;\n");
    let errors = headers::validate(temp.path(), true).unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("missing author-owned header fields"))
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("crates/a/src/lib.rs")).unwrap(),
        "pub struct Missing;\n"
    );
}

#[test]
fn headers_rejects_dangling_runtime_edges() {
    let temp = tree();
    write(
        temp.path(),
        "contracts/runtime-edges.toml",
        "[[edge]]\nfrom='rust/crates/missing/src/lib.rs'\nto='rust/crates/other/src/lib.rs'\nkind='ffi'\nreason='missing source'\n",
    );
    assert!(
        headers::validate(temp.path(), false)
            .unwrap()
            .iter()
            .any(|e| e.contains("invalid runtime edge"))
    );
}

#[test]
fn size_counts_tests_and_build_scripts_not_only_library_sources() {
    let temp = tree();
    write(
        temp.path(),
        "crates/a/tests/large.rs",
        &"// line\n".repeat(501),
    );
    write(temp.path(), "bins/a/build.rs", &"// line\n".repeat(501));
    let errors = size::validate(temp.path(), "2026-10-19").unwrap();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().all(|e| e.contains("501 physical lines")));
}

fn review(root: &Path, reviewer: &str, expires: &str, lines: usize) {
    write(
        root,
        "contracts/modularity-reviews.toml",
        &format!(
            "[[review]]\npath='rust/crates/a/src/lib.rs'\nlines={lines}\ndecision='exception'\nreason='reviewed cohesion'\nauthor='phase-01'\nreviewer='{reviewer}'\nexpires='{expires}'\n"
        ),
    );
}

#[test]
fn size_requires_current_independent_exact_review() {
    let temp = tree();
    write(temp.path(), "crates/a/src/lib.rs", &"// line\n".repeat(501));
    review(temp.path(), "phase-02", "2026-10-20", 501);
    assert!(
        size::validate(temp.path(), "2026-10-19")
            .unwrap()
            .is_empty()
    );
    review(temp.path(), "phase-01", "2026-10-20", 501);
    assert!(
        size::validate(temp.path(), "2026-10-19")
            .unwrap()
            .iter()
            .any(|e| e.contains("non-independent"))
    );
    review(temp.path(), "phase-02", "2026-10-18", 500);
    let errors = size::validate(temp.path(), "2026-10-19").unwrap();
    assert!(errors.iter().any(|e| e.contains("expired")));
    assert!(errors.iter().any(|e| e.contains("actual 501")));
    write(temp.path(), "crates/a/src/lib.rs", "// short\n");
    assert!(
        size::validate(temp.path(), "2026-10-19")
            .unwrap()
            .iter()
            .any(|e| e.contains("stale review"))
    );
}

#[test]
fn fixtures_require_real_case_metadata_and_response_shapes() {
    assert!(!fixtures::validate_family(&json!({"cases":[]})).is_empty());
    let valid = json!({"family":"health", "baseline":"TypeScript", "cases":[{
        "id":"health#ok", "seq":1, "server":"nopass",
        "request":{"method":"GET", "path":"/api/health", "auth":"valid"},
        "response":{"status":200, "headers":{}, "kind":"json", "body":{"healthy":true}}
    }]});
    assert!(fixtures::validate_family(&valid).is_empty());
    let mut invalid = valid;
    invalid["cases"][0]["response"]["status"] = json!(999);
    assert!(
        fixtures::validate_family(&invalid)
            .iter()
            .any(|e| e.contains("malformed expected response"))
    );
}

#[test]
fn empty_fixture_corpus_cannot_pass() {
    let temp = tree();
    let errors = fixtures::validate(temp.path()).unwrap();
    assert!(errors.iter().any(|e| e.contains("zero differential cases")));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("no versioned JSON fixtures"))
    );
}

#[test]
fn actual_versioned_fixture_corpus_is_nonempty_and_well_formed() {
    let root = xtask::workspace::rust_root().unwrap();
    let errors = fixtures::validate(&root).unwrap();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn inventory_rejects_unmatched_stale_duplicate_and_unknown_coverage() {
    let found = BTreeSet::from([(
        "http".into(),
        "GET /health".into(),
        "packages/api.ts".into(),
        3,
    )]);
    let row = inventory::Entry {
        kind: "http".into(),
        name: "GET /health".into(),
        source: "packages/api.ts".into(),
        line: 3,
        coverage: "HTTP-01".into(),
        status: "unverified".into(),
    };
    assert!(
        inventory::compare(&found, std::slice::from_ref(&row), "| HTTP-01 | health |").is_empty()
    );
    assert!(
        inventory::compare(&found, &[], "HTTP-01")
            .iter()
            .any(|e| e.contains("unmatched"))
    );
    let errors = inventory::compare(&found, &[row.clone(), row.clone()], "HTTP-010");
    assert!(errors.iter().any(|e| e.contains("duplicate")));
    assert!(errors.iter().any(|e| e.contains("unknown coverage")));
    let stale = inventory::Entry { line: 4, ..row };
    assert!(
        inventory::compare(&found, &[stale], "HTTP-01")
            .iter()
            .any(|e| e.contains("stale"))
    );
}

#[test]
fn actual_baseline_discovery_covers_each_registry_family() {
    let root = xtask::workspace::rust_root().unwrap();
    let found = inventory::discover(root.parent().unwrap()).unwrap();
    for kind in ["http", "tool", "provider", "cli", "event"] {
        assert!(found.iter().any(|row| row.0 == kind), "missing {kind}");
    }
    assert!(found.iter().filter(|row| row.0 == "http").count() >= 188);
}

#[test]
fn aggregate_reports_every_failure_and_does_not_stop_early() {
    let gates: [check::Gate; 3] = [
        ("first", |_| anyhow::bail!("first failure")),
        ("middle", |_| Ok(())),
        ("last", |_| anyhow::bail!("last failure")),
    ];
    let error = check::evaluate(&gates).unwrap_err().to_string();
    assert!(error.contains("first, last"), "{error}");
    assert!(check::run(&["--skip".into()]).is_err());
}
