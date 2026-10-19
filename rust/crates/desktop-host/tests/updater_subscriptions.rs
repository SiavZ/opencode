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
use std::sync::atomic::{AtomicBool, Ordering};

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
fn clear_visits_reentrant_insertions_like_js_map() {
    let subscriptions = Arc::new(UpdaterSubscriptions::new());
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let reentrant = subscriptions.clone();
    let first = disposed.clone();
    subscriptions.set(1, move || {
        assert_eq!(reentrant.len(), 2);
        first.lock().push(1);
        let inserted = first.clone();
        reentrant.set(3, move || inserted.lock().push(3));
    });
    let second = disposed.clone();
    subscriptions.set(2, move || second.lock().push(2));

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

#[test]
fn replacement_callback_observes_old_entry_before_new_one_is_installed() {
    let subscriptions = Arc::new(UpdaterSubscriptions::new());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let reentrant = subscriptions.clone();
    let first = observed.clone();
    subscriptions.set(1, move || first.lock().push(reentrant.len()));
    subscriptions.set(1, || {});
    assert_eq!(*observed.lock(), vec![1]);
    assert_eq!(subscriptions.len(), 1);
}

#[test]
fn clear_skips_removed_entries_and_visits_their_replacements() {
    let subscriptions = Arc::new(UpdaterSubscriptions::new());
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let reentrant = subscriptions.clone();
    let first = disposed.clone();
    subscriptions.set(1, move || {
        first.lock().push(1);
        reentrant.delete(&2);
        let inserted = first.clone();
        reentrant.set(2, move || inserted.lock().push(20));
    });
    let second = disposed.clone();
    subscriptions.set(2, move || second.lock().push(2));
    let third = disposed.clone();
    subscriptions.set(3, move || third.lock().push(3));
    subscriptions.clear();
    assert_eq!(*disposed.lock(), vec![1, 2, 3, 20]);
    assert!(subscriptions.is_empty());
}

#[test]
fn nested_delete_invokes_callback_again_before_removing_reentrant_replacement() {
    let subscriptions = Arc::new(UpdaterSubscriptions::new());
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(AtomicBool::new(false));
    let reentrant = subscriptions.clone();
    let observed = disposed.clone();
    subscriptions.set(1, move || {
        observed.lock().push(1);
        if !entered.swap(true, Ordering::SeqCst) {
            let replacement = observed.clone();
            reentrant.set(1, move || replacement.lock().push(10));
        }
    });
    subscriptions.delete(&1);
    subscriptions.clear();
    assert_eq!(*disposed.lock(), vec![1, 1]);
    assert!(subscriptions.is_empty());
}

#[test]
fn panicking_callback_does_not_delete_its_map_entry() {
    let subscriptions = UpdaterSubscriptions::new();
    subscriptions.set(1, || panic!("callback failure"));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            subscriptions.delete(&1);
        }))
        .is_err()
    );
    assert_eq!(subscriptions.len(), 1);
}
