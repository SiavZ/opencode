//! Purpose: test the updater subscriptions map.
//! Scope: unit tests for `UpdaterSubscriptions`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! External:
//! |-- opencode_desktop_host::updater::UpdaterSubscriptions
//! |-- parking_lot::Mutex
//! `-- std::sync::Arc
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover all TS `updater-subscriptions.test.ts` cases.
//! Contract tests: desktop-host/tests/updater_subscriptions.rs

use opencode_desktop_host::updater::UpdaterSubscriptions;
use parking_lot::Mutex;
use std::sync::Arc;

#[test]
fn replaces_the_previous_renderer_subscription_on_reload() {
    let subscriptions = UpdaterSubscriptions::new();
    let disposed = Arc::new(Mutex::new(Vec::new()));

    let disposed_clone = disposed.clone();
    subscriptions.set(1, move || {
        disposed_clone.lock().push("first".to_string());
    });

    let disposed_clone = disposed.clone();
    subscriptions.set(1, move || {
        disposed_clone.lock().push("second".to_string());
    });

    assert_eq!(*disposed.lock(), vec!["first".to_string()]);
    subscriptions.delete(&1);
    assert_eq!(
        *disposed.lock(),
        vec!["first".to_string(), "second".to_string()]
    );
}

#[test]
fn clears_all_subscriptions() {
    let subscriptions = UpdaterSubscriptions::new();
    let disposed = Arc::new(Mutex::new(Vec::new()));

    let disposed_clone = disposed.clone();
    subscriptions.set(1, move || {
        disposed_clone.lock().push("first".to_string());
    });

    let disposed_clone = disposed.clone();
    subscriptions.set(2, move || {
        disposed_clone.lock().push("second".to_string());
    });

    assert_eq!(subscriptions.len(), 2);
    subscriptions.clear();
    assert_eq!(subscriptions.len(), 0);
    assert_eq!(
        *disposed.lock(),
        vec!["first".to_string(), "second".to_string()]
    );
}
