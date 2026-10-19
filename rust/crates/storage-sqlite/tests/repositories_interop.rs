//! Purpose: live interop proof for the repositories against the real TypeScript services: rows written
//! by the Rust repositories are read back by `Credential`, `PermissionSaved` and `ProjectDirectories`
//! (packages/core), and rows those TS services write are read by the Rust repositories.
//! Scope: live tests. Spawns `bun` from packages/core with HOME/XDG and OPENCODE_DB pointed into a temp
//! dir. Skipped with a message when bun or packages/core/node_modules are unavailable.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! |-- storage-sqlite/src/lib.rs
//! |-- storage-sqlite/src/repositories/credentials.rs
//! |-- storage-sqlite/src/repositories/mod.rs
//! `-- storage-sqlite/tests/support/mod.rs
//! External:
//! |-- credentials::create
//! |-- credentials::list
//! |-- permission::add
//! |-- permission::list
//! |-- project::insert
//! |-- project_directory::Behavior::Ignore
//! |-- project_directory::create
//! |-- project_directory::list
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches the real user data directory.
//! Contract tests: storage-sqlite/tests/repositories_interop.rs

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use opencode_storage_api::{Database, DatabaseExt, TxOptions};
use opencode_storage_sqlite::SqliteDatabase;
use opencode_storage_sqlite::repositories::credentials::{self, CredentialValue};
use opencode_storage_sqlite::repositories::{permission, project, project_directory};
use serde_json::{Value, json};
use support::{R, project_row};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn available() -> bool {
    let core = repo().join("packages/core/node_modules/effect");
    let bun = Command::new("bun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !(bun && core.exists()) {
        eprintln!(
            "SKIP repositories_interop: bun or packages/core/node_modules missing (live test not run)"
        );
    }
    bun && core.exists()
}

fn ts(home: &Path, db: &Path, mode: &str) -> Value {
    let output = Command::new("bun")
        .current_dir(repo().join("packages/core"))
        .arg(repo().join("rust/crates/storage-sqlite/tests/ts/repos.ts"))
        .arg(mode)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("OPENCODE_DB", db)
        .output()
        .expect("spawn bun");
    assert!(
        output.status.success(),
        "bun failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(stdout.lines().last().unwrap_or("null")).unwrap()
}

#[tokio::test]
async fn rust_and_ts_repositories_share_rows() {
    if !available() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("opencode.db");

    let db = SqliteDatabase::open(&file).await.unwrap();
    db.transaction(TxOptions::default(), |sql| -> R<()> {
        project::insert(sql, &project_row("prj_rust"))?;
        project::insert(sql, &project_row("prj_ts"))?;
        credentials::create(
            sql,
            "openai",
            &CredentialValue::Key {
                key: "sk-rust".into(),
                metadata: Some(json!({"org": "o"}).as_object().unwrap().clone()),
            },
            Some("From Rust"),
        )?;
        permission::add(sql, "prj_rust", "edit", &["src/**".into()])?;
        project_directory::create(
            sql,
            "prj_rust",
            "/rust/dir",
            Some("worktree"),
            project_directory::Behavior::Ignore,
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.close().await.unwrap();

    // TS reads what Rust wrote.
    let read = ts(home.path(), &file, "read");
    let credential = &read["credentials"][0];
    assert!(credential["id"].as_str().unwrap().starts_with("cred_"));
    assert_eq!(credential["integrationID"], json!("openai"));
    assert_eq!(credential["label"], json!("From Rust"));
    assert_eq!(
        credential["value"],
        json!({"type": "key", "key": "sk-rust", "metadata": {"org": "o"}})
    );
    let perm = &read["permissions"][0];
    assert_eq!(
        (
            perm["projectID"].clone(),
            perm["action"].clone(),
            perm["resource"].clone()
        ),
        (json!("prj_rust"), json!("edit"), json!("src/**"))
    );
    assert!(perm["id"].as_str().unwrap().starts_with("psv_"));
    assert_eq!(
        read["directories"],
        json!([{"directory": "/rust/dir", "strategy": "worktree"}])
    );

    // Rust reads what TS wrote (credential rotation replaces nothing here: other integration).
    ts(home.path(), &file, "write");
    let db = SqliteDatabase::open(&file).await.unwrap();
    let (creds, perms, dirs) = db
        .read(|sql| -> R<_> {
            Ok((
                credentials::list(sql, "anthropic")?,
                permission::list(sql, Some("prj_ts"))?,
                project_directory::list(sql, "prj_ts")?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(creds.len(), 1);
    assert_eq!(creds[0].label, "From TS");
    assert_eq!(
        creds[0].value.expose(),
        &CredentialValue::Oauth {
            method_id: "device".into(),
            refresh: "r".into(),
            access: "a".into(),
            expires: 7,
            metadata: None,
        }
    );
    let mut resources: Vec<&str> = perms.iter().map(|p| p.resource.as_str()).collect();
    resources.sort_unstable();
    assert_eq!(resources, ["git *", "ls"]);
    assert_eq!(dirs.len(), 1);
    assert_eq!(
        (dirs[0].directory.as_str(), dirs[0].strategy.as_deref()),
        ("/ts/dir", Some("s"))
    );
    db.close().await.unwrap();
}
