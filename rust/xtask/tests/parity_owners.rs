//! Purpose: verify owner-specific census artifacts partition every captured requirement.
//! Scope: read-only baseline fixtures and generated Markdown.
//! Uses:
//! rust/xtask/
//! |-- src/parity.rs
//! `-- src/workspace.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: ownership filters never drop or duplicate requirements across the seven artifacts.
//! Contract tests: this file

#[test]
fn owner_reports_form_a_complete_disjoint_partition_and_preserve_fixture_hashes() {
    let root = xtask::workspace::rust_root().unwrap();
    let full = xtask::parity::render(&root).unwrap();
    let mut rows = std::collections::BTreeSet::new();
    for owner in [
        "api",
        "session",
        "native",
        "integration",
        "provider",
        "cli",
        "composition",
    ] {
        let report = xtask::parity::render_for(&root, Some(owner)).unwrap();
        assert!(
            report.lines().count() < 500,
            "{owner}: report must stay readable"
        );
        for hash in full.lines().filter(|line| line.contains("SHA-256:")) {
            assert!(report.contains(hash));
        }
        for row in report
            .lines()
            .filter(|line| line.starts_with("| ") && line.contains(" | `"))
        {
            assert!(
                rows.insert(row.to_owned()),
                "duplicate requirement in {owner}: {row}"
            );
        }
    }
    assert_eq!(rows.len(), 430);
    assert_eq!(
        rows,
        full.lines()
            .filter(|line| line.starts_with("| ") && line.contains(" | `"))
            .map(String::from)
            .collect()
    );
}
