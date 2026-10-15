//! Purpose: per-renderer updater subscription bookkeeping, port of
//! `packages/desktop/src/main/updater-subscriptions.ts`. A reload replaces (and disposes) the
//! previous subscription for the same webview id.
//! Scope: in-memory map, no IO.
//! Uses:
//! `-- none
//! External:
//! |-- parking_lot::Mutex
//! `-- std::collections::HashMap
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/updater.rs
//! `-- desktop-host/tests/updater_controller.rs
//! rust/bins/
//! `-- desktop/src/updater.rs
//! Invariants: at most one live unsubscribe per id, every stored unsubscribe runs exactly once.
//! Contract tests: desktop-host/tests/updater_controller.rs

use std::collections::HashMap;
use std::hash::Hash;

use parking_lot::Mutex;

type Unsubscribe = Box<dyn FnOnce() + Send>;

pub struct UpdaterSubscriptions<K = u64> {
    inner: Mutex<HashMap<K, Unsubscribe>>,
}

impl<K> std::fmt::Debug for UpdaterSubscriptions<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterSubscriptions")
            .field("len", &self.inner.lock().len())
            .finish()
    }
}

impl<K: Eq + Hash> Default for UpdaterSubscriptions<K> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash> UpdaterSubscriptions<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `unsubscribe` for `id`, disposing any previous subscription for the same id first.
    pub fn set(&self, id: K, unsubscribe: impl FnOnce() + Send + 'static) {
        let previous = self.inner.lock().insert(id, Box::new(unsubscribe));
        // Run outside the lock: an unsubscribe may re-enter the map.
        if let Some(previous) = previous {
            previous();
        }
    }

    /// Dispose and forget the subscription for `id`. No-op when absent.
    pub fn delete(&self, id: &K) {
        let removed = self.inner.lock().remove(id);
        if let Some(removed) = removed {
            removed();
        }
    }

    /// Dispose every subscription (app `will-quit`).
    pub fn clear(&self) {
        let drained: Vec<_> = self.inner.lock().drain().map(|(_, v)| v).collect();
        drained.into_iter().for_each(|unsubscribe| unsubscribe());
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
