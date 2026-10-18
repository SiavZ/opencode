//! Purpose: shared fixtures for the repository conformance tests (migrated in-memory database,
//! transaction shortcut, raw row dumps, sample project and session rows).
//! Scope: test support module, included with `mod support;`.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! `-- serde_json
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/tests/repositories.rs
//! |-- storage-sqlite/tests/repositories_interop.rs
//! `-- storage-sqlite/tests/repositories_accounts.rs
//! Invariants: every database is a fresh migrated in-memory `SqliteDatabase`.
//! Contract tests: storage-sqlite/tests/repositories.rs
#![allow(dead_code)]

use opencode_storage_api::records::{ProjectRow, SessionRow};
use opencode_storage_api::{DatabaseExt, Sql, StorageError, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::json;

pub type R<T> = Result<T, StorageError>;

pub async fn db() -> SqliteDatabase {
    SqliteDatabase::open_in_memory().await.unwrap()
}

pub async fn tx<T: Send + 'static>(
    db: &SqliteDatabase,
    work: impl FnOnce(&dyn Sql) -> R<T> + Send + 'static,
) -> R<T> {
    db.transaction(TxOptions::default(), work).await
}

pub fn raw(sql: &dyn Sql, query: &str) -> Vec<Vec<Value>> {
    sql.query(query, &[])
        .unwrap()
        .into_iter()
        .map(|r| r.into_values())
        .collect()
}

pub fn project_row(id: &str) -> ProjectRow {
    ProjectRow {
        id: id.into(),
        worktree: "/work/tree".into(),
        vcs: Some("git".into()),
        name: None,
        icon_url: None,
        icon_url_override: None,
        icon_color: None,
        time_created: 1,
        time_updated: 2,
        time_initialized: None,
        sandboxes: vec!["/work/sb".into()],
        commands: Some(json!({"start": "bun dev"})),
    }
}

pub fn session_row(id: &str, project_id: &str, directory: &str) -> SessionRow {
    SessionRow {
        id: id.into(),
        project_id: project_id.into(),
        workspace_id: None,
        parent_id: None,
        slug: "slug".into(),
        directory: directory.into(),
        path: None,
        title: "t".into(),
        version: "1".into(),
        share_url: None,
        summary_additions: None,
        summary_deletions: None,
        summary_files: None,
        summary_diffs: None,
        metadata: None,
        cost: 0.0,
        tokens_input: 0,
        tokens_output: 0,
        tokens_reasoning: 0,
        tokens_cache_read: 0,
        tokens_cache_write: 0,
        revert: None,
        permission: None,
        agent: None,
        model: None,
        time_created: 5,
        time_updated: 6,
        time_compacting: None,
        time_archived: None,
    }
}
