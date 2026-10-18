//! Purpose: conformance tests for the legacy JSON store, ported from
//! packages/opencode/test/storage/storage.test.ts:63-297 (round trip, NotFound, update, overwrite,
//! remove, list, concurrency, deep keys, migrations 1 and 2, failed migration marker).
//! Scope: live tests on temp data dirs. Migration 1 uses a real `git` repository and is skipped with a
//! message when `git` is unavailable.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- serde_json
//! |-- tempfile
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches the real user data dir.
//! Contract tests: storage-sqlite/tests/legacy_json.rs

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use opencode_storage_api::json_store::key;
use opencode_storage_api::{JsonStore, StorageError};
use opencode_storage_sqlite::legacy_files::LegacyJsonStore;
use serde_json::{Value, json};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn store() -> (tempfile::TempDir, LegacyJsonStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = LegacyJsonStore::new(dir.path());
    (dir, store)
}

// "round-trips JSON content" + files are JSON.stringify(value, null, 2)
#[tokio::test]
async fn round_trips_json_content() {
    let (dir, svc) = store();
    let k = key(["session_diff", "roundtrip"]);
    let value = json!([{"file": "a.ts", "additions": 2, "deletions": 1}]);
    svc.write(&k, &value).await.unwrap();
    assert_eq!(svc.read(&k).await.unwrap(), value);
    let text =
        std::fs::read_to_string(dir.path().join("storage/session_diff/roundtrip.json")).unwrap();
    assert_eq!(
        text,
        "[\n  {\n    \"file\": \"a.ts\",\n    \"additions\": 2,\n    \"deletions\": 1\n  }\n]"
    );
}

// "maps missing reads to NotFoundError" (message contains the file path)
#[tokio::test]
async fn missing_reads_are_not_found() {
    let (dir, svc) = store();
    let error = svc.read(&key(["missing", "value"])).await.unwrap_err();
    let expected = dir.path().join("storage/missing/value.json");
    match &error {
        StorageError::NotFound(path) => assert_eq!(Path::new(path), expected),
        other => panic!("expected NotFound, got {other:?}"),
    }
    assert!(error.to_string().starts_with("Resource not found: "));
}

// "update on missing key throws NotFoundError"
#[tokio::test]
async fn update_missing_is_not_found() {
    let (_dir, svc) = store();
    let error = svc
        .update(
            &key(["missing", "key"]),
            Box::new(|v| v["value"] = json!(1)),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::NotFound(_)));
}

// "write overwrites existing value", "remove on missing key is a no-op", "list on missing prefix returns empty"
#[tokio::test]
async fn overwrite_remove_missing_and_empty_list() {
    let (_dir, svc) = store();
    let k = key(["overwrite", "test"]);
    svc.write(&k, &json!({"v": 1})).await.unwrap();
    svc.write(&k, &json!({"v": 2})).await.unwrap();
    assert_eq!(svc.read(&k).await.unwrap(), json!({"v": 2}));
    svc.remove(&key(["nonexistent", "key"])).await.unwrap();
    assert!(svc.list(&key(["nonexistent"])).await.unwrap().is_empty());
}

// "serializes concurrent updates for the same key" + "concurrent reads do not block each other"
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_serialize() {
    let (_dir, svc) = store();
    let svc = Arc::new(svc);
    let k = key(["counter", "shared"]);
    svc.write(&k, &json!({"value": 0})).await.unwrap();
    let tasks: Vec<_> = (0..25)
        .map(|_| {
            let (svc, k) = (svc.clone(), k.clone());
            tokio::spawn(async move {
                svc.update(
                    &k,
                    Box::new(|v| {
                        let next = v["value"].as_i64().unwrap() + 1;
                        v["value"] = json!(next);
                    }),
                )
                .await
                .unwrap()
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(svc.read(&k).await.unwrap(), json!({"value": 25}));
    let reads: Vec<_> = (0..10)
        .map(|_| {
            let (svc, k) = (svc.clone(), k.clone());
            tokio::spawn(async move { svc.read(&k).await.unwrap() })
        })
        .collect();
    for read in reads {
        assert_eq!(read.await.unwrap(), json!({"value": 25}));
    }
}

// "nested keys create deep paths" + "lists and removes stored entries"
#[tokio::test]
async fn deep_keys_list_and_remove() {
    let (_dir, svc) = store();
    let deep = key(["a", "b", "c", "deep"]);
    svc.write(&deep, &json!({"nested": true})).await.unwrap();
    assert_eq!(svc.list(&key(["a"])).await.unwrap(), [deep]);
    let a = key(["list", "a"]);
    let b = key(["list", "b"]);
    svc.write(&b, &json!({"value": 2})).await.unwrap();
    svc.write(&a, &json!({"value": 1})).await.unwrap();
    assert_eq!(
        svc.list(&key(["list"])).await.unwrap(),
        [a.clone(), b.clone()]
    );
    svc.remove(&a).await.unwrap();
    assert_eq!(svc.list(&key(["list"])).await.unwrap(), [b]);
    assert!(svc.read(&a).await.is_err());
}

// list sorts by joined key with localeCompare (case-insensitive primary, `_` before `-`), not bytes.
// Names avoid case-only collisions so the test also holds on case-insensitive file systems.
#[tokio::test]
async fn list_uses_locale_compare() {
    let (_dir, svc) = store();
    for id in ["c", "B", "a", "a-x", "a_x", "Ab"] {
        svc.write(&key(["sort", id]), &json!(1)).await.unwrap();
    }
    let names: Vec<String> = svc
        .list(&key(["sort"]))
        .await
        .unwrap()
        .into_iter()
        .map(|k| k[1].clone())
        .collect();
    assert_eq!(names, ["a", "a_x", "a-x", "Ab", "B", "c"]);
}

// "migration 2 runs when marker contents are invalid"
#[tokio::test]
async fn migration_2_with_invalid_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = tmp.path().join("storage");
    write(&storage.join("migration"), "wat");
    write(
        &storage.join("session/proj_test/ses_test.json"),
        r#"{"id":"ses_test","projectID":"proj_test","title":"legacy","summary":{"diffs":[{"additions":2,"deletions":1},{"additions":3,"deletions":4}]}}"#,
    );
    let svc = LegacyJsonStore::new(tmp.path());
    assert_eq!(
        svc.list(&key(["session_diff"])).await.unwrap(),
        [key(["session_diff", "ses_test"])]
    );
    assert_eq!(
        svc.read(&key(["session_diff", "ses_test"])).await.unwrap(),
        json!([{"additions": 2, "deletions": 1}, {"additions": 3, "deletions": 4}])
    );
    assert_eq!(
        svc.read(&key(["session", "proj_test", "ses_test"]))
            .await
            .unwrap(),
        json!({"id": "ses_test", "projectID": "proj_test", "title": "legacy", "summary": {"additions": 5, "deletions": 5}})
    );
    assert_eq!(
        std::fs::read_to_string(storage.join("migration")).unwrap(),
        "2"
    );
}

// Migration 2 skips sessions whose summary fails the schema (negative counts) but still advances.
#[tokio::test]
async fn migration_2_skips_invalid_summaries() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = tmp.path().join("storage");
    write(&storage.join("migration"), "1");
    let original =
        r#"{"id":"ses_x","projectID":"p","summary":{"diffs":[{"additions":-1,"deletions":0}]}}"#;
    write(&storage.join("session/p/ses_x.json"), original);
    let svc = LegacyJsonStore::new(tmp.path());
    assert!(svc.list(&key(["session_diff"])).await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(storage.join("session/p/ses_x.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read_to_string(storage.join("migration")).unwrap(),
        "2"
    );
}

fn git_available() -> bool {
    let ok = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("SKIP migration 1 test: git unavailable");
    }
    ok
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

// "migration 1 tolerates malformed legacy records"
#[tokio::test]
async fn migration_1_tolerates_malformed_records() {
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    git(
        &root,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=T",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "root",
        ],
    );
    let legacy = root.join("project/legacy");
    write(&legacy.join("storage/session/message/probe/0.json"), "[]");
    write(
        &legacy.join("storage/session/message/probe/1.json"),
        &json!({"path": {"root": root}}).to_string(),
    );
    write(
        &legacy.join("storage/session/info/ses_legacy.json"),
        r#"{"id":"ses_legacy","title":"legacy"}"#,
    );
    write(
        &legacy.join("storage/session/message/ses_legacy/msg_legacy.json"),
        r#"{"role":"user","text":"hello"}"#,
    );
    let svc = LegacyJsonStore::new(&root);
    let projects = svc.list(&key(["project"])).await.unwrap();
    assert_eq!(projects.len(), 1);
    let project = projects[0][1].clone();
    assert_eq!(project.len(), 40, "project id is the root commit sha");
    let info = svc.read(&key(["project", project.as_str()])).await.unwrap();
    assert_eq!(info["vcs"], json!("git"));
    assert_eq!(info["worktree"], json!(root));
    assert_eq!(
        svc.list(&key(["session", project.as_str()])).await.unwrap(),
        [key(["session", project.as_str(), "ses_legacy"])]
    );
    assert_eq!(
        svc.read(&key(["session", project.as_str(), "ses_legacy"]))
            .await
            .unwrap(),
        json!({"id": "ses_legacy", "title": "legacy"})
    );
    assert_eq!(
        svc.read(&key(["message", "ses_legacy", "msg_legacy"]))
            .await
            .unwrap(),
        json!({"role": "user", "text": "hello"})
    );
    assert_eq!(
        std::fs::read_to_string(root.join("storage/migration")).unwrap(),
        "2"
    );
}

// "failed migrations do not advance the marker"
#[tokio::test]
async fn failed_migration_keeps_marker() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        &tmp.path()
            .join("project/legacy/storage/session/message/probe/0.json"),
        "{",
    );
    let svc = LegacyJsonStore::new(tmp.path());
    assert!(svc.list(&key(["project"])).await.unwrap().is_empty());
    assert!(!tmp.path().join("storage/migration").exists());
}

// The store implements the portable contract object-safely.
#[tokio::test]
async fn usable_as_dyn_json_store() {
    let (_dir, svc) = store();
    let svc: Arc<dyn JsonStore> = Arc::new(svc);
    let out: Value = svc
        .update(&key(["x"]), Box::new(|_| {}))
        .await
        .map(|_| Value::Null)
        .unwrap_or(json!("missing"));
    assert_eq!(out, json!("missing"));
}
