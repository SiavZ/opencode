//! Purpose: port of store-cleanup.test.ts plus edge cases (fixture-based temp dirs).
//! Scope: integration test through `opencode_desktop_host::store::cleanup`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/store/cleanup.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data.
//! Contract tests: desktop-host/tests/store_cleanup.rs

use std::path::Path;
use std::time::{Duration, SystemTime};

use opencode_desktop_host::store::cleanup::{cleanup_store_files, delete_store_file_if_empty};

// 2026-07-01T00:00:00Z
const NOW: u64 = 1_782_864_000;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn write_store(root: &Path, name: &str, value: &str, modified: SystemTime) {
    std::fs::write(root.join(name), value).unwrap();
    std::fs::File::options()
        .write(true)
        .open(root.join(name))
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

fn listing(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn removes_empty_scoped_stores_and_leaves_global_stores_alone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_store(root, "opencode.draft.empty.dat", "{}", at(NOW));
    write_store(root, "opencode.workspace.empty.dat", "{\n}", at(NOW));
    write_store(root, "opencode.global.dat", "{}", at(NOW));
    write_store(root, "opencode.workspace.empty.dat.json", "{}", at(NOW));
    let mut result = cleanup_store_files(root, at(NOW));
    result.deleted.sort();
    assert_eq!(
        result.deleted,
        ["opencode.draft.empty.dat", "opencode.workspace.empty.dat"]
    );
    assert_eq!(
        listing(root),
        ["opencode.global.dat", "opencode.workspace.empty.dat.json"]
    );
}

#[test]
fn removes_stale_drafts_by_age_without_removing_non_empty_workspace_stores() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_store(
        root,
        "opencode.draft.old.dat",
        r#"{"draft:prompt":"hello"}"#,
        at(NOW - 61 * 86_400),
    );
    write_store(
        root,
        "opencode.draft.recent.dat",
        r#"{"draft:prompt":"hello"}"#,
        at(NOW),
    );
    write_store(
        root,
        "opencode.workspace.old.dat",
        r#"{"workspace:layout":"wide"}"#,
        at(NOW - 547 * 86_400),
    );
    write_store(
        root,
        "opencode.workspace.recent.dat",
        r#"{"workspace:layout":"wide"}"#,
        at(NOW),
    );
    let result = cleanup_store_files(root, at(NOW));
    assert_eq!(result.deleted, ["opencode.draft.old.dat"]);
    assert_eq!(result.scanned, 4);
    assert_eq!(
        listing(root),
        [
            "opencode.draft.recent.dat",
            "opencode.workspace.old.dat",
            "opencode.workspace.recent.dat"
        ]
    );
}

#[test]
fn caps_scoped_stores_by_recency() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for index in 0..102 {
        write_store(
            root,
            &format!("opencode.draft.{index}.dat"),
            r#"{"draft:prompt":"hello"}"#,
            at(NOW - index),
        );
    }
    let mut result = cleanup_store_files(root, at(NOW));
    result.deleted.sort();
    assert_eq!(
        result.deleted,
        ["opencode.draft.100.dat", "opencode.draft.101.dat"]
    );
    assert_eq!(listing(root).len(), 100);
}

#[test]
fn removes_a_scoped_store_immediately_when_it_becomes_empty() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_store(root, "opencode.draft.empty.dat", "{}", at(NOW));
    write_store(root, "opencode.global.dat", "{}", at(NOW));
    assert!(delete_store_file_if_empty(root, "opencode.draft.empty.dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.global.dat"));
    assert_eq!(listing(root), ["opencode.global.dat"]);
}

#[test]
fn emptiness_rules_match_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_store(root, "opencode.draft.ws.dat", " \n\t", at(NOW));
    write_store(root, "opencode.draft.arr.dat", "[]", at(NOW));
    write_store(root, "opencode.draft.bad.dat", "{", at(NOW));
    let big = format!("{{{}}}", " ".repeat(200));
    write_store(root, "opencode.draft.big.dat", &big, at(NOW));
    write_store(root, "opencode.draft..dat", "{}", at(NOW));
    std::fs::create_dir(root.join("opencode.draft.dir.dat")).unwrap();
    assert!(delete_store_file_if_empty(root, "opencode.draft.ws.dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.draft.arr.dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.draft.bad.dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.draft.big.dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.draft..dat"));
    assert!(!delete_store_file_if_empty(root, "opencode.draft.dir.dat"));
    assert!(!delete_store_file_if_empty(
        root,
        "opencode.draft.missing.dat"
    ));
    // arr, bad and big remain. `opencode.draft..dat` does not match `.+` and dirs are skipped.
    let result = cleanup_store_files(root, at(NOW));
    assert_eq!(result.scanned, 3);
    assert!(result.deleted.is_empty());
}

#[test]
fn missing_directory_scans_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let result = cleanup_store_files(&dir.path().join("nope"), at(NOW));
    assert_eq!(result.scanned, 0);
}
