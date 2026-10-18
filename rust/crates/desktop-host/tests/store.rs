//! Purpose: contract tests for electron-store compatible named stores (fixture-based).
//! Scope: integration test through `opencode_desktop_host::store::StoreRegistry`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/src/store/keys.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data, fixtures mirror files written by conf 15.1.0.
//! Contract tests: desktop-host/tests/store.rs

use opencode_desktop_host::store::{SETTINGS_STORE, StoreError, StoreRegistry, to_js_json};
use serde_json::json;

fn registry() -> (tempfile::TempDir, StoreRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreRegistry::new(dir.path());
    (dir, stores)
}

#[test]
fn writes_conf_compatible_tab_indented_file_without_extension() {
    let (dir, stores) = registry();
    stores
        .set(SETTINGS_STORE, "defaultServerUrl", json!("http://x"))
        .unwrap();
    stores
        .set(SETTINGS_STORE, "windowIds", json!(["a", "b"]))
        .unwrap();
    stores
        .set(SETTINGS_STORE, "pinchZoomEnabled", json!(true))
        .unwrap();
    let raw = std::fs::read_to_string(dir.path().join("opencode.settings")).unwrap();
    // Byte-for-byte what JSON.stringify(value, undefined, '\t') produces.
    assert_eq!(
        raw,
        "{\n\t\"defaultServerUrl\": \"http://x\",\n\t\"windowIds\": [\n\t\t\"a\",\n\t\t\"b\"\n\t],\n\t\"pinchZoomEnabled\": true\n}"
    );
}

#[test]
fn reads_existing_electron_file_and_preserves_internal_key() {
    let (dir, stores) = registry();
    std::fs::write(
        dir.path().join("opencode.global.dat"),
        "{\n\t\"__internal__\": {\n\t\t\"migrations\": {\n\t\t\t\"version\": \"1.2.3\"\n\t\t}\n\t},\n\t\"language\": \"{\\\"locale\\\":\\\"de\\\"}\"\n}",
    )
    .unwrap();
    assert_eq!(
        stores.ipc_get("opencode.global.dat", "language").as_deref(),
        Some("{\"locale\":\"de\"}")
    );
    stores.ipc_clear("opencode.global.dat").unwrap();
    let after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("opencode.global.dat")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        after,
        json!({"__internal__": {"migrations": {"version": "1.2.3"}}})
    );
    assert!(matches!(
        stores.set("opencode.global.dat", "__internal__", json!(1)),
        Err(StoreError::ReservedKey)
    ));
    assert!(matches!(
        stores.set("opencode.global.dat", "x", json!({"__internal__.a": 1})),
        Err(StoreError::ReservedKey)
    ));
}

#[test]
fn ipc_get_stringifies_non_strings_and_maps_errors_to_null() {
    let (dir, stores) = registry();
    stores.set("default.dat", "n", json!(1)).unwrap();
    stores
        .set("default.dat", "o", json!({"b": 1, "a": [true, null]}))
        .unwrap();
    stores.set("default.dat", "z", json!(null)).unwrap();
    assert_eq!(stores.ipc_get("default.dat", "n").as_deref(), Some("1"));
    assert_eq!(
        stores.ipc_get("default.dat", "o").as_deref(),
        Some(r#"{"b":1,"a":[true,null]}"#)
    );
    assert_eq!(stores.ipc_get("default.dat", "z"), None);
    assert_eq!(stores.ipc_get("default.dat", "missing"), None);
    std::fs::write(dir.path().join("broken.dat"), "{not json").unwrap();
    assert_eq!(stores.ipc_get("broken.dat", "x"), None);
    assert!(matches!(
        stores.set("broken.dat", "x", json!(1)),
        Err(StoreError::Corrupt { .. })
    ));
    // A missing store is empty, not an error.
    assert_eq!(stores.keys("nothing.dat").unwrap(), Vec::<String>::new());
    assert_eq!(stores.len("nothing.dat").unwrap(), 0);
}

#[test]
fn keys_follow_js_property_order_and_proto_keys_are_ignored() {
    let (_dir, stores) = registry();
    for key in ["b", "10", "a", "2", "constructor", "__proto__"] {
        stores.ipc_set("default.dat", key, "v").unwrap();
    }
    assert_eq!(stores.keys("default.dat").unwrap(), ["2", "10", "b", "a"]);
    assert_eq!(stores.len("default.dat").unwrap(), 4);
    stores.ipc_delete("default.dat", "b").unwrap();
    assert_eq!(stores.keys("default.dat").unwrap(), ["2", "10", "a"]);
}

#[test]
fn delete_and_clear_remove_empty_scoped_store_files_only() {
    let (dir, stores) = registry();
    stores
        .ipc_set("opencode.draft.x.1.dat", "draft:prompt", "hi")
        .unwrap();
    stores.ipc_set("opencode.global.dat", "k", "v").unwrap();
    stores
        .ipc_delete("opencode.draft.x.1.dat", "draft:prompt")
        .unwrap();
    assert!(!dir.path().join("opencode.draft.x.1.dat").exists());
    stores.ipc_clear("opencode.global.dat").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("opencode.global.dat")).unwrap(),
        "{}"
    );
    stores.remove_file("opencode.global.dat").unwrap();
    stores.remove_file("opencode.global.dat").unwrap();
    assert!(!dir.path().join("opencode.global.dat").exists());
}

#[test]
fn rejects_store_names_that_escape_user_data() {
    let (_dir, stores) = registry();
    for name in ["../x", "a/b", "", "..", "a\\b"] {
        assert!(
            matches!(
                stores.ipc_set(name, "k", "v"),
                Err(StoreError::InvalidName(_))
            ),
            "{name}"
        );
        assert_eq!(stores.ipc_get(name, "k"), None);
    }
}

#[test]
fn js_json_formatting_matches_node() {
    assert_eq!(to_js_json(&json!({})), "{}");
    assert_eq!(to_js_json(&json!([])), "[]");
    assert_eq!(
        to_js_json(&json!({"a": {}, "b": 1.0, "1": "x"})),
        "{\n\t\"1\": \"x\",\n\t\"a\": {},\n\t\"b\": 1\n}"
    );
    assert_eq!(
        to_js_json(&json!({"s": "é\u{2028}<"})),
        "{\n\t\"s\": \"é\u{2028}<\"\n}"
    );
}

#[test]
fn concurrent_writers_do_not_lose_keys() {
    let (_dir, stores) = registry();
    let stores = std::sync::Arc::new(stores);
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let stores = stores.clone();
            std::thread::spawn(move || {
                for j in 0..10 {
                    stores
                        .ipc_set("default.dat", &format!("k{i}-{j}"), "v")
                        .unwrap();
                }
            })
        })
        .collect();
    handles.into_iter().for_each(|h| h.join().unwrap());
    assert_eq!(stores.len("default.dat").unwrap(), 80);
}
