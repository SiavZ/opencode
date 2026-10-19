//! Purpose: test the updater file store.
//! Scope: unit tests for `UpdaterFileStore`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! External:
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover channel persistence, ready record, and empty store cleanup.
//! Contract tests: desktop-host/tests/updater_store.rs

use opencode_desktop_host::updater::{
    UpdaterError, UpdaterFileStore, UpdaterPersistence, UpdaterReadyRecord,
};
use tempfile::TempDir;

#[test]
fn persists_and_reads_ready_record() {
    let dir = TempDir::new().unwrap();
    let store = UpdaterFileStore::new(dir.path());
    assert!(store.get().unwrap().is_none());

    let record = UpdaterReadyRecord {
        version: "2.0.0".to_string(),
    };
    store.set(&record).unwrap();
    assert_eq!(store.get().unwrap().unwrap().version, "2.0.0");

    store.clear().unwrap();
    assert!(store.get().unwrap().is_none());
}

#[test]
fn persists_and_reads_channel() {
    let dir = TempDir::new().unwrap();
    let store = UpdaterFileStore::new(dir.path());
    assert_eq!(store.channel(), "latest");

    store.set_channel("beta").unwrap();
    assert_eq!(store.channel(), "beta");
}

#[test]
fn rejects_invalid_channel() {
    let dir = TempDir::new().unwrap();
    let store = UpdaterFileStore::new(dir.path());
    assert!(matches!(
        store.set_channel("invalid channel!"),
        Err(UpdaterError::InvalidChannel(_))
    ));
}

#[test]
fn removes_file_when_store_becomes_empty() {
    let dir = TempDir::new().unwrap();
    let store = UpdaterFileStore::new(dir.path());
    let record = UpdaterReadyRecord {
        version: "2.0.0".to_string(),
    };
    store.set(&record).unwrap();
    assert!(store.path().exists());

    store.clear().unwrap();
    assert!(!store.path().exists());
}

#[test]
fn handles_corrupt_file_gracefully() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode.updater");
    std::fs::write(&path, "not json").unwrap();

    let store = UpdaterFileStore::new(dir.path());
    assert!(store.get().unwrap().is_none());
    assert_eq!(store.channel(), "latest");
}
