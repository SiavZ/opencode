//! Purpose: test the updater subscriptions map.
//! Scope: unit tests for `UpdaterSubscriptions`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! External:
//! `-- parking_lot::Mutex
//! Used by:
//! `-- none, entrypoint
//! Invariants: disposal follows baseline JS Map insertion order; replacement moves an id to the end.
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
fn clear_preserves_insertion_order_after_replacement_and_reinsertion() {
    let subscriptions = UpdaterSubscriptions::new();
    let disposed = Arc::new(Mutex::new(Vec::new()));

    for id in [30, 10, 20] {
        let disposed = disposed.clone();
        subscriptions.set(id, move || disposed.lock().push(id));
    }

    let replaced = disposed.clone();
    subscriptions.set(30, move || replaced.lock().push(300));
    assert_eq!(*disposed.lock(), vec![30]);

    subscriptions.delete(&10);
    subscriptions.delete(&10);
    assert_eq!(*disposed.lock(), vec![30, 10]);
    let reinserted = disposed.clone();
    subscriptions.set(10, move || reinserted.lock().push(100));

    subscriptions.clear();
    subscriptions.clear();
    assert!(subscriptions.is_empty());
    assert_eq!(*disposed.lock(), vec![30, 10, 20, 300, 100]);
}

#[test]
fn clear_preserves_rust_snapshot_safety_for_reentrant_insertions() {
    // Preserve the existing Rust snapshot behavior: callbacks run after draining and unlocking.
    // JS Map.forEach visits insertions made during iteration, so this is not a parity assertion.
    let subscriptions = Arc::new(UpdaterSubscriptions::new());
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let reentrant = subscriptions.clone();
    let first = disposed.clone();
    subscriptions.set(1, move || {
        assert!(reentrant.is_empty());
        first.lock().push(1);
        reentrant.set(3, move || first.lock().push(3));
    });
    let second = disposed.clone();
    subscriptions.set(2, move || second.lock().push(2));

    subscriptions.clear();
    assert_eq!(*disposed.lock(), vec![1, 2]);
    assert_eq!(subscriptions.len(), 1);
    subscriptions.clear();
    assert_eq!(*disposed.lock(), vec![1, 2, 3]);
    assert!(subscriptions.is_empty());
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
