//! Purpose: executable SQL persistence, row formats and typed failure regressions.
//! Scope: disposable database only.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: no live user database or data is queried or mutated.
//! Contract tests: self

use super::super::support::{Home, assert_success, output};
use serde_json::{Value, json};

#[test]
fn explicit_database_queries_persist_and_format_null_and_blob_values() {
    let home = Home::new();
    let created = output(
        home.command(&["db", "CREATE TABLE cli_fixture (name TEXT, count INTEGER)"]),
        None,
    );
    assert_success(&created);
    let inserted = output(
        home.command(&["db", "INSERT INTO cli_fixture VALUES ('fixture', 7)"]),
        None,
    );
    assert_success(&inserted);
    let queried = output(
        home.command(&[
            "db",
            "SELECT name,count,NULL AS empty,x'0102' AS bytes FROM cli_fixture",
            "--format",
            "json",
        ]),
        None,
    );
    assert_success(&queried);
    let rows: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert_eq!(
        rows,
        json!([{"name":"fixture","count":7,"empty":null,"bytes":{"0":1,"1":2}}])
    );
    let queried = output(
        home.command(&["db", "SELECT name,count,NULL AS empty FROM cli_fixture"]),
        None,
    );
    assert_success(&queried);
    assert_eq!(
        String::from_utf8_lossy(&queried.stdout),
        "name\tcount\tempty\nfixture\t7\t\n"
    );
    let invalid = output(
        home.command(&["db", "SELECT * FROM nonexistent_cli_fixture"]),
        None,
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("nonexistent_cli_fixture"));
}
