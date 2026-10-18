//! Purpose: tests for Tauri-era and Electron user data migration using temp-dir fixtures built
//! from the baseline layouts (`*.dat` JSON, conf store files, `drafts.sqlite`).
//! Scope: integration test through `opencode_desktop_host::migrate`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/drafts.rs
//! |-- desktop-host/src/migrate.rs
//! `-- desktop-host/src/store.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real ~/Library or XDG data, directory resolution uses injected env.
//! Contract tests: desktop-host/tests/migrate.rs

use std::path::{Path, PathBuf};

use opencode_desktop_host::drafts::DraftStore;
use opencode_desktop_host::migrate::*;
use opencode_desktop_host::store::{SETTINGS_STORE, StoreRegistry};
use serde_json::json;

#[test]
fn electron_import_preserves_updater_store_and_source_wal_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("electron");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("opencode.updater"),
        r#"{"ready":{"version":"2.0.0"},"channel":"latest"}"#,
    )
    .unwrap();
    let source_store = DraftStore::open_in(&source).unwrap();
    let id = source_store.put_blob(b"wal-image").unwrap();
    source_store.set("prompt", Some(format!(r#"{{"blob":{{"id":"{id}"}}}}"#)));
    source_store.flush().unwrap();
    assert!(source.join("drafts.sqlite-wal").exists());
    let source_bytes = std::fs::read(source.join("opencode.updater")).unwrap();
    let target = StoreRegistry::new(dir.path().join("target"));
    target
        .set("opencode.updater", "channel", json!("beta"))
        .unwrap();
    let MigrationOutcome::Migrated {
        draft_documents, ..
    } = migrate_electron(&source, &target).unwrap()
    else {
        panic!("expected import")
    };
    assert_eq!(draft_documents, 1);
    assert_eq!(
        target.get("opencode.updater", "channel").unwrap(),
        Some(json!("beta"))
    );
    assert_eq!(
        target.get("opencode.updater", "ready").unwrap(),
        Some(json!({"version":"2.0.0"}))
    );
    let imported = DraftStore::open_in(target.user_data()).unwrap();
    assert_eq!(imported.get_blob(&id).unwrap().unwrap(), b"wal-image");
    assert_eq!(
        std::fs::read(source.join("opencode.updater")).unwrap(),
        source_bytes
    );
    assert!(source_store.get("prompt").unwrap().is_some());
    assert_eq!(
        migrate_electron(&source, &target).unwrap(),
        MigrationOutcome::AlreadyDone
    );
}

#[test]
fn tauri_non_object_entries_match_javascript_and_preserve_index_order() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tauri");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("array.dat"), r#"["first",{"a":true}]"#).unwrap();
    std::fs::write(source.join("string.dat"), r#""abé""#).unwrap();
    std::fs::write(source.join("number.dat"), "42").unwrap();
    std::fs::write(source.join("boolean.dat"), "false").unwrap();
    let stores = StoreRegistry::new(dir.path().join("target"));
    let MigrationOutcome::Migrated { failed, files, .. } = migrate_tauri(&source, &stores).unwrap()
    else {
        panic!("expected import")
    };
    assert!(failed.is_empty());
    assert_eq!(files.len(), 4);
    assert_eq!(stores.keys("array.dat").unwrap(), ["0", "1"]);
    assert_eq!(
        stores.get("array.dat", "1").unwrap(),
        Some(json!({"a":true}))
    );
    assert_eq!(stores.get("string.dat", "2").unwrap(), Some(json!("é")));
    assert_eq!(stores.len("number.dat").unwrap(), 0);
    assert_eq!(stores.len("boolean.dat").unwrap(), 0);
}

#[test]
fn null_tauri_source_fails_without_marking_migration_complete() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tauri");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("null.dat"), "null").unwrap();
    let stores = StoreRegistry::new(dir.path().join("target"));
    assert!(matches!(
        migrate_tauri(&source, &stores),
        Err(MigrateError::NullSource { .. })
    ));
    assert_eq!(stores.get(SETTINGS_STORE, "tauriMigrated").unwrap(), None);
    std::fs::write(source.join("null.dat"), r#"{"fixed":true}"#).unwrap();
    migrate_tauri(&source, &stores).unwrap();
    assert_eq!(stores.get("null.dat", "fixed").unwrap(), Some(json!(true)));
}

#[test]
fn surrogate_pair_string_migration_reports_explicit_unsupported_without_flag() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tauri");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("string.dat"), r#""😀""#).unwrap();
    let stores = StoreRegistry::new(dir.path().join("target"));
    assert!(matches!(
        migrate_tauri(&source, &stores),
        Err(MigrateError::Unsupported(_))
    ));
    assert_eq!(stores.get(SETTINGS_STORE, "tauriMigrated").unwrap(), None);
}

#[test]
fn resolves_tauri_and_electron_dirs_per_os() {
    let env = DirEnv {
        home: PathBuf::from("/h"),
        ..Default::default()
    };
    let id = Channel::Prod.app_id(true);
    assert_eq!(id, "ai.opencode.desktop");
    assert_eq!(Channel::Prod.app_id(false), "ai.opencode.desktop.dev");
    assert_eq!(Channel::Beta.app_id(true), "ai.opencode.desktop.beta");
    assert_eq!(
        tauri_dir(Os::Mac, &env, id),
        Path::new("/h/Library/Application Support/ai.opencode.desktop")
    );
    assert_eq!(
        tauri_dir(Os::Windows, &env, id),
        Path::new("/h/AppData/Roaming/ai.opencode.desktop")
    );
    assert_eq!(
        tauri_dir(Os::Linux, &env, id),
        Path::new("/h/.local/share/ai.opencode.desktop")
    );
    let env = DirEnv {
        home: PathBuf::from("/h"),
        appdata: Some("/ad".into()),
        xdg_data_home: Some("/xd".into()),
        xdg_config_home: Some("/xc".into()),
    };
    assert_eq!(
        tauri_dir(Os::Windows, &env, id),
        Path::new("/ad/ai.opencode.desktop")
    );
    assert_eq!(
        tauri_dir(Os::Linux, &env, id),
        Path::new("/xd/ai.opencode.desktop")
    );
    assert_eq!(
        electron_user_data_dir(Os::Linux, &env, id),
        Path::new("/xc/ai.opencode.desktop")
    );
    assert_eq!(
        electron_user_data_dir(Os::Mac, &env, id),
        Path::new("/h/Library/Application Support/ai.opencode.desktop")
    );
}

#[test]
fn tauri_migration_merges_without_overwriting_and_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let tauri = dir.path().join("tauri");
    std::fs::create_dir_all(&tauri).unwrap();
    std::fs::write(
        tauri.join("opencode.settings.dat"),
        r#"{"defaultServerUrl":"http://old","pinchZoomEnabled":true}"#,
    )
    .unwrap();
    std::fs::write(
        tauri.join("opencode.global.dat"),
        r#"{"language":"{\"locale\":\"fr\"}"}"#,
    )
    .unwrap();
    std::fs::write(tauri.join("broken.dat"), "{oops").unwrap();
    std::fs::write(tauri.join("ignored.json"), r#"{"a":1}"#).unwrap();
    let stores = StoreRegistry::new(dir.path().join("user"));
    stores
        .set(SETTINGS_STORE, "defaultServerUrl", json!("http://new"))
        .unwrap();

    let outcome = migrate_tauri(&tauri, &stores).unwrap();
    let MigrationOutcome::Migrated { files, failed, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(failed, ["broken.dat"]);
    let settings = files
        .iter()
        .find(|f| f.file == "opencode.settings.dat")
        .unwrap();
    assert_eq!(settings.store, "opencode.settings");
    assert_eq!(settings.migrated, ["pinchZoomEnabled"]);
    assert_eq!(settings.skipped, ["defaultServerUrl"]);
    assert_eq!(
        stores.get(SETTINGS_STORE, "defaultServerUrl").unwrap(),
        Some(json!("http://new"))
    );
    assert_eq!(
        stores.ipc_get("opencode.global.dat", "language").as_deref(),
        Some(r#"{"locale":"fr"}"#)
    );
    assert_eq!(
        stores.get(SETTINGS_STORE, "tauriMigrated").unwrap(),
        Some(json!(true))
    );
    assert!(!dir.path().join("user/ignored.json").exists());

    std::fs::write(tauri.join("late.dat"), r#"{"x":1}"#).unwrap();
    assert_eq!(
        migrate_tauri(&tauri, &stores).unwrap(),
        MigrationOutcome::AlreadyDone
    );
    assert!(!dir.path().join("user/late.dat").exists());
    // Source files are untouched.
    assert!(tauri.join("opencode.settings.dat").exists());
}

#[test]
fn tauri_migration_without_source_sets_flag() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreRegistry::new(dir.path().join("user"));
    assert_eq!(
        migrate_tauri(&dir.path().join("missing"), &stores).unwrap(),
        MigrationOutcome::NoSource
    );
    assert_eq!(
        stores.get(SETTINGS_STORE, "tauriMigrated").unwrap(),
        Some(json!(true))
    );
}

fn electron_fixture(root: &Path) -> (String, String) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("opencode.settings"),
        "{\n\t\"windowIds\": [\n\t\t\"w1\"\n\t],\n\t\"tauriMigrated\": true,\n\t\"defaultServerUrl\": \"http://electron\"\n}",
    )
    .unwrap();
    std::fs::write(
        root.join("opencode.window.w1.dat"),
        "{\n\t\"tabs\": \"[]\"\n}",
    )
    .unwrap();
    std::fs::write(
        root.join("window-state-w1.json"),
        r#"{"width":800,"height":600}"#,
    )
    .unwrap();
    std::fs::write(root.join("Local State"), "{}").unwrap();
    let drafts = DraftStore::open_in(root).unwrap();
    let image = drafts.put_blob(b"png").unwrap();
    let orphan = drafts.put_blob(b"orphan").unwrap();
    drafts.set(
        "draft:1",
        Some(format!(
            r#"{{"parts":[{{"type":"image","blob":{{"id":"{image}"}}}}]}}"#
        )),
    );
    drafts.set("draft:2", Some(r#"{"text":"electron"}"#.into()));
    drafts.close().unwrap();
    (image, orphan)
}

#[test]
fn electron_import_copies_stores_window_state_and_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let electron = dir.path().join("electron");
    let (image, orphan) = electron_fixture(&electron);
    let stores = StoreRegistry::new(dir.path().join("native"));
    stores
        .set(SETTINGS_STORE, "defaultServerUrl", json!("http://native"))
        .unwrap();
    {
        let target = DraftStore::open_in(stores.user_data()).unwrap();
        target.set("draft:2", Some(r#"{"text":"native"}"#.into()));
        target.close().unwrap();
    }

    let outcome = migrate_electron(&electron, &stores).unwrap();
    let MigrationOutcome::Migrated {
        files,
        failed,
        draft_documents,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(failed.is_empty());
    assert_eq!(draft_documents, 1);
    assert_eq!(files.len(), 2);
    assert_eq!(
        stores.get(SETTINGS_STORE, "defaultServerUrl").unwrap(),
        Some(json!("http://native"))
    );
    assert_eq!(
        stores.get(SETTINGS_STORE, "windowIds").unwrap(),
        Some(json!(["w1"]))
    );
    assert_eq!(
        stores.ipc_get("opencode.window.w1.dat", "tabs").as_deref(),
        Some("[]")
    );
    assert!(stores.user_data().join("window-state-w1.json").exists());
    assert!(!stores.user_data().join("Local State").exists());

    let drafts = DraftStore::open_in(stores.user_data()).unwrap();
    assert_eq!(
        drafts.get("draft:2").unwrap().as_deref(),
        Some(r#"{"text":"native"}"#)
    );
    assert!(drafts.get("draft:1").unwrap().is_some());
    assert_eq!(
        drafts.get_blob(&image).unwrap().as_deref(),
        Some(&b"png"[..])
    );
    // Unreferenced blobs were already collected by the source store's own open/close cycle or
    // are collected on the first target open.
    assert_eq!(drafts.get_blob(&orphan).unwrap(), None);
    drafts.close().unwrap();

    assert_eq!(
        migrate_electron(&electron, &stores).unwrap(),
        MigrationOutcome::AlreadyDone
    );
}

#[test]
fn electron_import_in_place_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let electron = dir.path().join("electron");
    electron_fixture(&electron);
    let before = std::fs::read_to_string(electron.join("opencode.window.w1.dat")).unwrap();
    let stores = StoreRegistry::new(&electron);
    assert_eq!(
        migrate_electron(&electron, &stores).unwrap(),
        MigrationOutcome::NoSource
    );
    assert_eq!(
        std::fs::read_to_string(electron.join("opencode.window.w1.dat")).unwrap(),
        before
    );
    // Existing Electron data reads in place.
    assert_eq!(
        stores.get(SETTINGS_STORE, "defaultServerUrl").unwrap(),
        Some(json!("http://electron"))
    );
    assert_eq!(
        stores.get(SETTINGS_STORE, "electronMigrated").unwrap(),
        Some(json!(true))
    );
}
