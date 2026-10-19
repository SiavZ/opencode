//! Purpose: prevent ledger diagnostics from silently replacing strict production parity.
//! Scope: real command parser and baseline census, read-only.
//! Uses:
//! rust/xtask/
//! |-- src/fixtures.rs
//! |-- src/parity.rs
//! `-- src/workspace.rs
//! External:
//! |-- parity::render
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: strict is the execution default; conformance requires explicit selection.
//! Contract tests: this file

use serde_json::json;
use xtask::{fixtures, parity};

#[test]
fn execution_defaults_to_strict_and_conformance_cannot_mix_with_it() {
    assert_eq!(fixtures::mode(&[]).unwrap(), fixtures::Mode::Validate);
    assert_eq!(
        fixtures::mode(&["run".into()]).unwrap(),
        fixtures::Mode::Strict
    );
    assert_eq!(
        fixtures::mode(&["run".into(), "--strict".into()]).unwrap(),
        fixtures::Mode::Strict
    );
    assert_eq!(
        fixtures::mode(&["run".into(), "--conformance".into()]).unwrap(),
        fixtures::Mode::Conformance
    );
    for args in [
        vec!["--strict"],
        vec!["run", "--strict", "--conformance"],
        vec!["run", "--bless"],
    ] {
        assert!(fixtures::mode(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
    }
}

#[test]
fn census_counts_actual_baseline_and_assigns_every_requirement() {
    let root = xtask::workspace::rust_root().unwrap();
    let report = parity::render(&root).unwrap();
    assert!(report.contains("430 captured cases, 384 ledger mismatches, 46 matches"));
    assert!(report.contains("2ed84974fc779a5b5d06b65bb8618fc0d6a5e1006c81e00c72abf88d3d7cf4d7"));
    assert!(report.contains("a85c7751375bb0c144d7a1d45a160b7728c6222a18de6f97997324ab6133c9c7"));
    for (category, count) in [
        ("not-mounted", 222),
        ("not-ported", 140),
        ("status", 15),
        ("body", 7),
    ] {
        assert!(report.contains(&format!("| {category} | {count} |")));
    }
    assert_eq!(
        report
            .lines()
            .filter(|line| line.starts_with("| ") && line.contains(" | `") && line.contains(" | "))
            .count(),
        430
    );
    assert!(report.contains("NOT the production Process"));
    assert!(report.contains("--test strict_replay -- --ignored"));
}

#[test]
fn current_and_legacy_routes_share_domain_ownership() {
    for (route, owner) in [
        ("session", "session/cat"),
        ("pty", "native/chicken"),
        ("mcp", "integration/chick"),
        ("provider", "provider/bird"),
    ] {
        assert_eq!(parity::owner(&format!("/{route}/id")), owner);
        assert_eq!(parity::owner(&format!("/api/{route}/id")), owner);
    }
    assert_eq!(parity::owner("/doc"), "API/bat");
}

#[test]
fn census_rejects_orphaned_ledger_entries_and_duplicate_case_ids() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("crates/server/tests/fixtures");
    std::fs::create_dir_all(base.join("baseline")).unwrap();
    let case = json!({"id":"one", "seq":1, "request":{"method":"GET", "path":"/session"}});
    std::fs::write(
        base.join("baseline/one.json"),
        json!({"family":"session", "cases":[case]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        base.join("replay-ledger.json"),
        json!({"orphan":{"category":"body"}}).to_string(),
    )
    .unwrap();
    assert!(
        parity::render(temp.path())
            .unwrap_err()
            .to_string()
            .contains("orphan")
    );
    std::fs::write(base.join("replay-ledger.json"), "{}").unwrap();
    std::fs::write(
        base.join("baseline/one.json"),
        json!({"family":"session", "cases":[case, case]}).to_string(),
    )
    .unwrap();
    assert!(
        parity::render(temp.path())
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
}
