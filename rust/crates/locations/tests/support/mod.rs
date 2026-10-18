//! Purpose: shared fixtures for the locations integration tests: isolated real git repositories in
//! temp dirs, a migrated in-memory SQLite database and raw row helpers.
//! Scope: test support module, included with `mod support;`.
//! Uses:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/tests/support/workspace.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- tempfile
//! `-- tokio
//! Used by:
//! rust/crates/
//! |-- locations/tests/project.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- locations/tests/workspace_sync.rs
//! Invariants: git never reads the user's global or system config; nothing touches real user data.
//! Contract tests: locations/tests/project.rs
#![allow(dead_code)]

pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opencode_locations::git::{GitRunner, SubprocessGit};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;

pub fn git() -> Arc<SubprocessGit> {
    Arc::new(SubprocessGit {
        env: vec![
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_AUTHOR_NAME".into(), "t".into()),
            ("GIT_AUTHOR_EMAIL".into(), "t@example.com".into()),
            ("GIT_COMMITTER_NAME".into(), "t".into()),
            ("GIT_COMMITTER_EMAIL".into(), "t@example.com".into()),
        ],
    })
}

pub async fn run(dir: &Path, args: &[&str]) -> String {
    let out = git().run(dir, args).await;
    assert!(out.ok(), "git {args:?} failed: {}", out.stderr);
    out.text.trim().to_owned()
}

/// A canonical temp dir (macOS `/var` -> `/private/var`).
pub fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

/// `git init` with one commit. Returns the root commit hash.
pub async fn init_repo(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    run(dir, &["init", "-q", "-b", "main"]).await;
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "."]).await;
    run(dir, &["commit", "-q", "-m", "init"]).await;
    run(dir, &["rev-parse", "HEAD"]).await
}

pub async fn db() -> Arc<dyn Database> {
    Arc::new(SqliteDatabase::open_in_memory().await.unwrap())
}

pub async fn exec(db: &Arc<dyn Database>, sql: &'static str, params: Vec<Value>) {
    db.transaction(TxOptions::default(), move |tx: &dyn Sql| {
        tx.execute(sql, &params).map(|_| ())
    })
    .await
    .unwrap();
}

pub async fn rows(db: &Arc<dyn Database>, sql: &'static str) -> Vec<Vec<Value>> {
    db.read(move |tx: &dyn Sql| {
        Ok::<_, StorageError>(
            tx.query(sql, &[])?
                .into_iter()
                .map(|r| r.into_values())
                .collect(),
        )
    })
    .await
    .unwrap()
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Text(t) => t.clone(),
        other => panic!("not text: {other:?}"),
    }
}

pub async fn insert_project(db: &Arc<dyn Database>, id: &str, worktree: &str) {
    let (id, worktree) = (id.to_owned(), worktree.to_owned());
    db.transaction(TxOptions::default(), move |tx: &dyn Sql| {
        tx.execute(
            "INSERT INTO project (id, worktree, vcs, name, time_created, time_updated, sandboxes) VALUES (?1, ?2, 'git', 'named', 10, 20, '[]')",
            &[id.into(), worktree.into()],
        )
        .map(|_| ())
    })
    .await
    .unwrap();
}

pub async fn insert_session(
    db: &Arc<dyn Database>,
    id: &str,
    project: &str,
    directory: &str,
    workspace: Option<&str>,
    parent: Option<&str>,
) {
    let params: Vec<Value> = vec![
        id.into(),
        project.into(),
        workspace.map(Value::from).unwrap_or(Value::Null),
        parent.map(Value::from).unwrap_or(Value::Null),
        directory.into(),
    ];
    db.transaction(TxOptions::default(), move |tx: &dyn Sql| {
        tx.execute(
            "INSERT INTO session (id, project_id, workspace_id, parent_id, slug, directory, title, version, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 'slug', ?5, 't', '1', 1, 77)",
            &params,
        )
        .map(|_| ())
    })
    .await
    .unwrap();
}
