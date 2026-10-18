//! Purpose: contract tests for onboarding/old-layout flags and default server URL
//! (`main/onboarding.ts`, `main/install-state.ts` + test, `main/server.ts:30-42`).
//! Scope: in-memory settings store fixture plus tempdirs for user data and documents.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lifecycle/onboarding.rs
//! External:
//! |-- serde_json
//! `-- tempfile
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data.
//! Contract tests: this file

use std::collections::HashMap;

use opencode_desktop_host::lifecycle::onboarding::*;
use serde_json::{Value, json};

#[derive(Default)]
struct MemoryStore(HashMap<String, Value>);

impl SettingsStore for MemoryStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).cloned()
    }
    fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_string(), value);
    }
    fn delete(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn file(name: &str) -> DirEntryInfo {
    DirEntryInfo {
        name: name.into(),
        is_dir: false,
    }
}

fn directory(name: &str) -> DirEntryInfo {
    DirEntryInfo {
        name: name.into(),
        is_dir: true,
    }
}

// install-state.test.ts:8
#[test]
fn ignores_files_electron_may_create_on_a_fresh_install() {
    assert!(!has_existing_app_state(&[]));
    assert!(!has_existing_app_state(&[
        file("Local State"),
        directory("Crashpad")
    ]));
}

// install-state.test.ts:13
#[test]
fn recognizes_state_written_by_an_earlier_opencode_launch() {
    assert!(has_existing_app_state(&[file("opencode.settings")]));
    assert!(has_existing_app_state(&[file("opencode.global.dat")]));
    assert!(has_existing_app_state(&[file("window-state-abc.json")]));
    assert!(has_existing_app_state(&[directory("opencode")]));
    assert!(!has_existing_app_state(&[file("opencode")]));
    assert!(!has_existing_app_state(&[file("window-state-.json")]));
}

#[test]
fn old_layout_eligibility_is_computed_once_and_sticky() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let user_data = tmp.path().join("userData");
    let mut store = MemoryStore::default();

    // Missing directory is a fresh install.
    assert!(!initialize_old_layout_eligibility(&mut store, &user_data).expect("init"));
    assert_eq!(store.get(OLD_LAYOUT_ELIGIBLE_KEY), Some(json!(false)));
    assert!(!is_old_layout_eligible(&store));

    // Later state does not flip the stored decision.
    std::fs::create_dir_all(&user_data).expect("mkdir");
    std::fs::write(user_data.join("opencode.settings"), "{}").expect("write");
    assert!(!initialize_old_layout_eligibility(&mut store, &user_data).expect("init"));

    // A fresh store over existing state is eligible.
    let mut upgraded = MemoryStore::default();
    assert!(initialize_old_layout_eligibility(&mut upgraded, &user_data).expect("init"));
    assert!(is_old_layout_eligible(&upgraded));

    // Non-boolean stored value is recomputed.
    let mut odd = MemoryStore::default();
    odd.set(OLD_LAYOUT_ELIGIBLE_KEY, json!("true"));
    assert!(initialize_old_layout_eligibility(&mut odd, &user_data).expect("init"));
}

#[test]
fn onboarding_pending_until_finished_and_creates_default_project_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut store = MemoryStore::default();
    assert!(is_first_launch_onboarding_pending(&store));
    store.set(FIRST_LAUNCH_ONBOARDING_COMPLETE_KEY, json!("true"));
    assert!(is_first_launch_onboarding_pending(&store));

    let project = finish_first_launch_onboarding(&mut store, tmp.path(), true).expect("finish");
    assert_eq!(project, Some(tmp.path().join("Default Project")));
    assert!(tmp.path().join("Default Project").is_dir());
    assert!(!is_first_launch_onboarding_pending(&store));

    assert_eq!(
        finish_first_launch_onboarding(&mut store, tmp.path(), true).expect("again"),
        None
    );
}

#[test]
fn onboarding_without_default_project_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut store = MemoryStore::default();
    assert_eq!(
        finish_first_launch_onboarding(&mut store, tmp.path(), false).expect("finish"),
        None
    );
    assert!(!tmp.path().join("Default Project").exists());
    assert!(!is_first_launch_onboarding_pending(&store));
}

#[test]
fn failed_default_project_creation_leaves_onboarding_pending() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let blocker = tmp.path().join("documents");
    std::fs::write(&blocker, "not a dir").expect("write");
    let mut store = MemoryStore::default();
    assert!(finish_first_launch_onboarding(&mut store, &blocker, true).is_err());
    assert!(is_first_launch_onboarding_pending(&store));
}

#[test]
fn default_server_url_get_set_delete() {
    let mut store = MemoryStore::default();
    assert_eq!(default_server_url(&store), None);
    set_default_server_url(&mut store, Some("http://remote:4096"));
    assert_eq!(
        default_server_url(&store).as_deref(),
        Some("http://remote:4096")
    );
    set_default_server_url(&mut store, Some(""));
    assert_eq!(store.get(DEFAULT_SERVER_URL_KEY), None);
    set_default_server_url(&mut store, Some("http://a"));
    set_default_server_url(&mut store, None);
    assert_eq!(store.get(DEFAULT_SERVER_URL_KEY), None);
    store.set(DEFAULT_SERVER_URL_KEY, json!(42));
    assert_eq!(default_server_url(&store), None);
}
