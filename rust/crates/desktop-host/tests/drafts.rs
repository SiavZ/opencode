//! Purpose: contract tests for the SQLite draft store (fixture-based, temp dirs only).
//! Scope: integration test through the public `opencode_desktop_host::drafts` API.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/drafts.rs
//! External:
//! |-- rusqlite::Connection
//! |-- tempfile::tempdir
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data.
//! Contract tests: desktop-host/tests/drafts.rs

use opencode_desktop_host::drafts::{DraftError, DraftStore, FLUSH_DELAY, stored_documents};
use rusqlite::Connection;

// Port of draft-store.test.ts:4-16.
#[test]
fn flushes_the_latest_buffered_draft_and_stores_blobs() {
    let store = DraftStore::open(":memory:").unwrap();
    store.set("prompt", Some("first".into()));
    store.set("prompt", Some("latest".into()));
    assert_eq!(store.get("prompt").unwrap().as_deref(), Some("latest"));
    store.flush().unwrap();
    assert_eq!(store.get("prompt").unwrap().as_deref(), Some("latest"));

    let id = store.put_blob(b"image").unwrap();
    assert_eq!(store.get_blob(&id).unwrap().as_deref(), Some(&b"image"[..]));
    store.close().unwrap();
}

#[test]
fn blob_ids_are_sha256_hex_and_idempotent() {
    let store = DraftStore::open(":memory:").unwrap();
    let id = store.put_blob(b"image").unwrap();
    assert_eq!(
        id,
        "6105d6cc76af400325e94d588ce511be5bfdbb73b437dc51eca43917d7a43e3d"
    );
    assert_eq!(store.put_blob(b"image").unwrap(), id);
    assert_eq!(store.get_blob("missing").unwrap(), None);
}

#[test]
fn pending_delete_hides_stored_value_until_flushed() {
    let dir = tempfile::tempdir().unwrap();
    let store = DraftStore::open_in(dir.path()).unwrap();
    store.set("a", Some("1".into()));
    store.flush().unwrap();
    store.delete("a");
    assert_eq!(store.get("a").unwrap(), None);
    assert_eq!(store.pending_len(), 1);
    assert_eq!(
        stored_documents(&store)
            .unwrap()
            .get("a")
            .map(String::as_str),
        Some("1")
    );
    store.flush().unwrap();
    assert!(stored_documents(&store).unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn joined_shutdown_flushes_before_the_timer_and_closes_all_clones() {
    let dir = tempfile::tempdir().unwrap();
    let store = DraftStore::open_in(dir.path()).unwrap();
    let clone = store.clone();
    store.set("quit", Some("true".into()));
    store.close_and_join().await.unwrap();
    assert!(matches!(clone.get("quit"), Err(DraftError::Closed)));
    tokio::time::sleep(FLUSH_DELAY * 2).await;
    let reopened = DraftStore::open_in(dir.path()).unwrap();
    assert_eq!(reopened.get("quit").unwrap().as_deref(), Some("true"));
}

#[tokio::test(start_paused = true)]
async fn timer_rearms_after_previous_flush_and_joined_flush_keeps_store_open() {
    let store = DraftStore::open(":memory:").unwrap();
    store.set("first", Some("1".into()));
    tokio::time::sleep(FLUSH_DELAY + std::time::Duration::from_millis(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.pending_len(), 0);
    store.set("second", Some("2".into()));
    tokio::time::sleep(FLUSH_DELAY + std::time::Duration::from_millis(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.pending_len(), 0);
    store.set("third", Some("3".into()));
    store.flush_and_join().await.unwrap();
    assert_eq!(stored_documents(&store).unwrap().len(), 3);
    store.close_and_join().await.unwrap();
}

#[test]
fn schema_and_wal_match_baseline_and_close_flushes() {
    let dir = tempfile::tempdir().unwrap();
    let store = DraftStore::open_in(dir.path()).unwrap();
    store.set("k", Some("\"v\"".into()));
    store.close().unwrap();
    assert!(matches!(store.get("other"), Err(DraftError::Closed)));

    let db = Connection::open(dir.path().join("drafts.sqlite")).unwrap();
    let mode: String = db
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let sql: Vec<String> = db
        .prepare("SELECT sql FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        sql,
        [
            "CREATE TABLE blob (id TEXT PRIMARY KEY, data BLOB NOT NULL)",
            "CREATE TABLE document (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        ]
    );
    let v: String = db
        .query_row("SELECT value FROM document WHERE key = 'k'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, "\"v\"");
}

#[test]
fn drop_flushes_pending_writes() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = DraftStore::open_in(dir.path()).unwrap();
        store.set("k", Some("1".into()));
    }
    let store = DraftStore::open_in(dir.path()).unwrap();
    assert_eq!(store.get("k").unwrap().as_deref(), Some("1"));
}

#[test]
fn reopen_collects_unreferenced_blobs_only() {
    let dir = tempfile::tempdir().unwrap();
    let (kept, nested, dropped);
    {
        let store = DraftStore::open_in(dir.path()).unwrap();
        kept = store.put_blob(b"kept").unwrap();
        nested = store.put_blob(b"nested").unwrap();
        dropped = store.put_blob(b"dropped").unwrap();
        store.set(
            "prompt",
            Some(format!(
                r#"{{"parts":[{{"type":"image","blob":{{"id":"{kept}"}}}}],"x":{{"y":[{{"blob":{{"id":"{nested}"}}}}]}}}}"#
            )),
        );
        // Blob shapes the baseline does not treat as references.
        store.set(
            "other",
            Some(format!(r#"{{"blob":"{dropped}","id":"{dropped}"}}"#)),
        );
        store.close().unwrap();
    }
    let store = DraftStore::open_in(dir.path()).unwrap();
    assert!(store.get_blob(&kept).unwrap().is_some());
    assert!(store.get_blob(&nested).unwrap().is_some());
    assert_eq!(store.get_blob(&dropped).unwrap(), None);
}

#[test]
fn corrupt_document_fails_open_without_deleting_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    {
        let store = DraftStore::open_in(dir.path()).unwrap();
        id = store.put_blob(b"x").unwrap();
        store.set("bad", Some("not json".into()));
        store.close().unwrap();
    }
    assert!(matches!(
        DraftStore::open_in(dir.path()),
        Err(DraftError::CorruptDocument { key, .. }) if key == "bad"
    ));
    let db = Connection::open(dir.path().join("drafts.sqlite")).unwrap();
    let n: i64 = db
        .query_row("SELECT count(*) FROM blob WHERE id = ?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test(start_paused = true)]
async fn timer_flushes_after_delay_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = DraftStore::open_in(dir.path()).unwrap();
    store.set("a", Some("1".into()));
    tokio::time::sleep(FLUSH_DELAY / 2).await;
    // A second write does not re-arm the timer (baseline `schedule` only arms when idle).
    store.set("b", Some("2".into()));
    assert_eq!(store.pending_len(), 2);
    tokio::time::sleep(FLUSH_DELAY / 2 + std::time::Duration::from_millis(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.pending_len(), 0);
    let docs = stored_documents(&store).unwrap();
    assert_eq!(docs.get("a").map(String::as_str), Some("1"));
    assert_eq!(docs.get("b").map(String::as_str), Some("2"));
}
