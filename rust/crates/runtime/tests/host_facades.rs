//! Purpose: exercise public host facades against real isolated SQLite and process restart.
//! Scope: explicit SQL command semantics used by the CLI.
//! Uses:
//! rust/crates/
//! `-- runtime/src/process.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, integration test entrypoint
//! Invariants: mutations commit durably; query errors roll back and do not poison later work.
//! Contract tests: runtime/tests/host_facades.rs

use opencode_runtime::{Process, ProcessOptions};
use serde_json::json;

#[tokio::test]
async fn explicit_sql_facade_mutates_returns_storage_classes_and_reopens() {
    let root = tempfile::tempdir().unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    assert!(process.database_path().is_file());
    assert!(process.database_query("CREATE TABLE facade_probe (id INTEGER PRIMARY KEY, text_value TEXT, blob_value BLOB, real_value REAL, null_value TEXT)".into()).await.unwrap().is_empty());
    assert_eq!(
        process
            .database_query(
                "INSERT INTO facade_probe VALUES (7, 'hello', X'0102FF', 1.25, NULL) RETURNING *"
                    .into()
            )
            .await
            .unwrap(),
        vec![
            json!({"id":7,"text_value":"hello","blob_value":{"0":1,"1":2,"2":255},"real_value":1.25,"null_value":null})
        ]
    );
    assert!(
        process
            .database_query("SELECT missing_column FROM facade_probe".into())
            .await
            .is_err()
    );
    assert_eq!(
        process
            .database_query("SELECT COUNT(*) AS count FROM facade_probe".into())
            .await
            .unwrap(),
        vec![json!({"count":1})]
    );
    process.shutdown().await;

    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    assert_eq!(
        process
            .database_query("SELECT id, text_value FROM facade_probe".into())
            .await
            .unwrap(),
        vec![json!({"id":7,"text_value":"hello"})]
    );
    process.shutdown().await;
}
