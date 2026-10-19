//! Purpose: live interop proof against the real TypeScript baseline: databases created and migrated by
//! TS open and read in Rust, Rust writes read back by TS, and Rust migration of a partially migrated TS
//! database produces byte-identical `sqlite_master` to TS migrating the same database.
//! Scope: live tests. Spawns `bun` from packages/core with HOME/XDG pointed at a temp dir; skipped with a
//! message when bun or packages/core/node_modules are unavailable.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- rusqlite::Connection
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches the real user data directory.
//! Contract tests: storage-sqlite/tests/ts_interop.rs

use std::path::{Path, PathBuf};
use std::process::Command;

use opencode_storage_api::{Database, DatabaseExt, StorageError, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;
use rusqlite::Connection;

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
        eprintln!("SKIP ts_interop: bun or packages/core/node_modules missing (live test not run)");
    }
    bun && core.exists()
}

/// Run the TS harness with an isolated HOME/XDG under `home`.
fn ts(home: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new("bun")
        .current_dir(repo().join("packages/core"))
        .arg(repo().join("rust/crates/storage-sqlite/tests/ts/create_db.ts"))
        .args(args)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("OPENCODE_DB")
        .output()
        .expect("spawn bun");
    assert!(
        output.status.success(),
        "bun failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(stdout.lines().last().unwrap_or("[]")).unwrap()
}

fn master(path: &Path) -> Vec<(String, String, Option<String>)> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test]
async fn ts_created_database_round_trips_through_rust() {
    if !available() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("ts.db");
    let file_str = file.to_str().unwrap();
    ts(home.path(), &["open", file_str]);

    // Rust opens the TS database: no migration runs again, PRAGMAs apply, reads work.
    let db = SqliteDatabase::open(&file).await.unwrap();
    let ids: Vec<String> = db
        .read(|tx| {
            tx.query("SELECT id FROM migration ORDER BY id", &[])?
                .iter()
                .map(|r| r.get("id"))
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(ids.len(), 38);
    assert_eq!(
        ids,
        opencode_storage_sqlite::migrations::ids()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );

    // Rust writes with Drizzle encodings; TS reads them back.
    db.transaction(TxOptions::immediate(), |tx| {
        tx.execute(
            "INSERT INTO project (id, worktree, vcs, time_created, time_updated, sandboxes, commands) VALUES (?1, ?2, 'git', 1, 2, ?3, ?4)",
            &["prj_rust".into(), "/work/tree".into(), Value::json(&["/work/sb"])?, Value::json(&serde_json::json!({"start": "bun dev"}))?],
        )?;
        tx.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated, cost) VALUES ('ses_rust', 'prj_rust', 'slug', '/work/tree', 'From Rust ✓', '1.0.0', 3, 4, 0.5)",
            &[],
        )
    })
    .await
    .unwrap();
    db.close().await.unwrap();

    let rows = ts(
        home.path(),
        &[
            "open",
            file_str,
            "SELECT p.id, p.sandboxes, p.commands, s.title, s.cost, s.tokens_input FROM project p JOIN session s ON s.project_id = p.id",
        ],
    );
    assert_eq!(
        rows,
        serde_json::json!([{"id": "prj_rust", "sandboxes": "[\"/work/sb\"]", "commands": "{\"start\":\"bun dev\"}", "title": "From Rust ✓", "cost": 0.5, "tokens_input": 0}])
    );
}

#[tokio::test]
async fn rust_created_database_opens_in_ts_with_identical_schema() {
    if !available() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let rust_file = home.path().join("rust.db");
    let ts_file = home.path().join("ts.db");
    let db = SqliteDatabase::open(&rust_file).await.unwrap();
    db.transaction(TxOptions::default(), |tx| {
        tx.execute("INSERT INTO project (id, worktree, time_created, time_updated, sandboxes) VALUES ('global', '/', 1, 1, '[]')", &[])
    })
    .await
    .unwrap();
    db.close().await.unwrap();
    ts(home.path(), &["open", ts_file.to_str().unwrap()]);
    assert_eq!(
        master(&rust_file),
        master(&ts_file),
        "fresh schema differs between Rust and TS"
    );
    let rows = ts(
        home.path(),
        &[
            "open",
            rust_file.to_str().unwrap(),
            "SELECT id, (SELECT count(*) FROM migration) AS n FROM project",
        ],
    );
    assert_eq!(rows, serde_json::json!([{"id": "global", "n": 38}]));
}

#[tokio::test]
async fn rust_migrates_partially_migrated_ts_database_like_ts() {
    if !available() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    for applied in ["1", "10", "24", "33"] {
        let by_ts = home.path().join(format!("ts-{applied}.db"));
        let by_rust = home.path().join(format!("rust-{applied}.db"));
        ts(home.path(), &["partial", by_ts.to_str().unwrap(), applied]);
        // bun leaves committed pages in the WAL at exit; copy it too or the copy looks empty.
        std::fs::copy(&by_ts, &by_rust).unwrap();
        let wal = |p: &Path| PathBuf::from(format!("{}-wal", p.display()));
        if wal(&by_ts).exists() {
            std::fs::copy(wal(&by_ts), wal(&by_rust)).unwrap();
        }
        ts(home.path(), &["open", by_ts.to_str().unwrap()]);
        let db = SqliteDatabase::open(&by_rust).await.unwrap();
        let count: Result<i64, StorageError> = db
            .read(|tx| {
                tx.query_row("SELECT count(*) AS n FROM migration", &[])?
                    .unwrap()
                    .get("n")
            })
            .await;
        assert_eq!(count.unwrap(), 38);
        db.close().await.unwrap();
        assert_eq!(
            master(&by_ts),
            master(&by_rust),
            "schema differs after migrating from step {applied}"
        );
    }
}
