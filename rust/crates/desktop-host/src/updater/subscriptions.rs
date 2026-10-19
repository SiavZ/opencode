//! Purpose: per-renderer updater subscription bookkeeping, port of
//! `packages/desktop/src/main/updater-subscriptions.ts`.
//! Scope: in-memory map, no IO.
//! Uses:
//! `-- none
//! External:
//! `-- parking_lot::Mutex
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! Invariants: callbacks run before deletion, outside locks; clear visits live insertion order.
//! Contract tests: desktop-host/tests/updater_subscriptions.rs

use std::hash::Hash;
use std::sync::Arc;

use parking_lot::Mutex;

type Unsubscribe = Arc<dyn Fn() + Send + Sync>;

struct Entries<K> {
    items: Vec<(u64, K, Unsubscribe)>,
    next: u64,
}

pub struct UpdaterSubscriptions<K = u64> {
    inner: Mutex<Entries<K>>,
}

impl<K> std::fmt::Debug for UpdaterSubscriptions<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterSubscriptions")
            .field("len", &self.inner.lock().items.len())
            .finish()
    }
}

impl<K: Eq + Hash> Default for UpdaterSubscriptions<K> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Entries {
                items: Vec::new(),
                next: 0,
            }),
        }
    }
}

impl<K: Eq + Hash> UpdaterSubscriptions<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dispose the old callback before installing its replacement, as JS `remove` does.
    pub fn set(&self, id: K, unsubscribe: impl Fn() + Send + Sync + 'static) {
        self.delete(&id);
        let mut inner = self.inner.lock();
        // Concurrent callers can insert between delete and lock acquisition. Map.set
        // overwrites that value without changing its insertion position.
        if let Some((_, _, callback)) = inner.items.iter_mut().find(|(_, key, _)| key == &id) {
            *callback = Arc::new(unsubscribe);
            return;
        }
        let next = inner.next;
        inner.next = next
            .checked_add(1)
            .expect("subscription insertion overflow");
        inner.items.push((next, id, Arc::new(unsubscribe)));
    }

    /// Invoke while the key is still present, then delete even a reentrant replacement.
    pub fn delete(&self, id: &K) {
        let callback = self
            .inner
            .lock()
            .items
            .iter()
            .find(|(_, key, _)| key == id)
            .map(|(_, _, callback)| callback.clone());
        if let Some(callback) = callback {
            callback();
        }
        self.inner.lock().items.retain(|(_, key, _)| key != id);
    }

    /// JS Map iteration skips removed entries and visits entries inserted by callbacks.
    pub fn clear(&self) {
        let mut cursor = None;
        loop {
            let entry = self
                .inner
                .lock()
                .items
                .iter()
                .find(|(sequence, _, _)| cursor.is_none_or(|previous| *sequence > previous))
                .map(|(sequence, _, callback)| (*sequence, callback.clone()));
            let Some((sequence, callback)) = entry else {
                self.inner.lock().items.clear();
                return;
            };
            cursor = Some(sequence);
            callback();
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
