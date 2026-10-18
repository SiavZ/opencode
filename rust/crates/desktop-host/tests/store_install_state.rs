//! Purpose: port of install-state.test.ts plus old-layout eligibility persistence.
//! Scope: integration test through `opencode_desktop_host::store::install_state`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/src/store/install_state.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data.
//! Contract tests: desktop-host/tests/store_install_state.rs

use opencode_desktop_host::store::StoreRegistry;
use opencode_desktop_host::store::install_state::{
    Entry, has_existing_app_state, initialize_old_layout_eligibility, is_old_layout_eligible,
};

fn file(name: &str) -> Entry {
    Entry {
        name: name.into(),
        is_dir: false,
    }
}
fn directory(name: &str) -> Entry {
    Entry {
        name: name.into(),
        is_dir: true,
    }
}

#[test]
fn ignores_files_electron_may_create_on_a_fresh_install() {
    assert!(!has_existing_app_state(&[]));
    assert!(!has_existing_app_state(&[
        file("Local State"),
        directory("Crashpad")
    ]));
    assert!(!has_existing_app_state(&[
        file("opencode"),
        file("window-state-.json")
    ]));
}

#[test]
fn recognizes_state_written_by_an_earlier_opencode_launch() {
    assert!(has_existing_app_state(&[file("opencode.settings")]));
    assert!(has_existing_app_state(&[file("opencode.global.dat")]));
    assert!(has_existing_app_state(&[file("window-state-abc.json")]));
    assert!(has_existing_app_state(&[directory("opencode")]));
}

#[test]
fn eligibility_is_computed_once_and_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = StoreRegistry::new(dir.path().join("fresh"));
    assert!(!initialize_old_layout_eligibility(&fresh).unwrap());
    // The settings file now exists, but the stored false wins.
    assert!(!initialize_old_layout_eligibility(&fresh).unwrap());
    assert!(!is_old_layout_eligible(&fresh));

    let existing = dir.path().join("existing");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("opencode.global.dat"), "{}").unwrap();
    let stores = StoreRegistry::new(&existing);
    assert!(initialize_old_layout_eligibility(&stores).unwrap());
    assert!(is_old_layout_eligible(&stores));
}
