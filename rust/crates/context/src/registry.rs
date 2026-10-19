//! Purpose: Location-scoped system context registry (`packages/core/src/system-context/registry.ts`):
//! scoped entry registration with duplicate entry-key rejection and `load` that re-evaluates every
//! entry producer and combines them in stable key order.
//! Scope: in-memory per Location. Registration lifetime is the returned guard.
//! Uses:
//! rust/crates/
//! `-- context/src/source/mod.rs
//! External:
//! |-- futures::future::try_join_all
//! `-- parking_lot::Mutex
//! Used by:
//! rust/crates/
//! |-- context/src/builtins.rs
//! |-- context/src/lib.rs
//! |-- context/tests/registry.rs
//! |-- runtime/src/location.rs
//! |-- session-runtime/src/runner/ports.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: entries are removed exactly when their `Registration` drops (baseline scope close),
//! load order is key order regardless of registration order, producer failures propagate,
//! duplicate source keys across entries fail with `DuplicateKey`.
//! Contract tests: context/tests/registry.rs

use std::sync::Arc;

use futures::future::try_join_all;
use parking_lot::Mutex;

use crate::source::{BoxFuture, ContextError, Key, SystemContext, combine};

type EntryLoad = Arc<dyn Fn() -> BoxFuture<Result<SystemContext, ContextError>> + Send + Sync>;

/// One registry entry: a key plus a producer re-run on every `load`.
#[derive(Clone)]
pub struct Entry {
    pub key: Key,
    pub load: EntryLoad,
}

impl Entry {
    pub fn new<F, Fut>(key: Key, load: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<SystemContext, ContextError>> + Send + 'static,
    {
        Self {
            key,
            load: Arc::new(move || Box::pin(load())),
        }
    }

    /// Entry whose producer always yields the same context.
    pub fn constant(key: Key, context: SystemContext) -> Self {
        Self::new(key, move || {
            let context = context.clone();
            async move { Ok(context) }
        })
    }
}

struct Slot {
    id: u64,
    entry: Entry,
}

#[derive(Default)]
struct Inner {
    next: u64,
    entries: Vec<Slot>,
}

/// Registry service. Cheap to clone, clones share state.
#[derive(Clone, Default)]
pub struct Registry {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_list()
            .entries(inner.entries.iter().map(|s| s.entry.key.as_str()))
            .finish()
    }
}

/// Keeps an entry registered. Dropping it unregisters (baseline `acquireRelease` finalizer).
#[must_use = "dropping the registration removes the entry"]
pub struct Registration {
    registry: Registry,
    id: u64,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry
            .inner
            .lock()
            .entries
            .retain(|slot| slot.id != self.id);
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an entry. Fails with `DuplicateEntry` while another entry holds the same key.
    pub fn register(&self, entry: Entry) -> Result<Registration, ContextError> {
        let mut inner = self.inner.lock();
        if inner.entries.iter().any(|slot| slot.entry.key == entry.key) {
            return Err(ContextError::DuplicateEntry { key: entry.key });
        }
        inner.next += 1;
        let id = inner.next;
        inner.entries.push(Slot { id, entry });
        Ok(Registration {
            registry: self.clone(),
            id,
        })
    }

    /// Re-evaluate every producer and combine them in stable key order.
    pub async fn load(&self) -> Result<SystemContext, ContextError> {
        let mut current: Vec<Entry> = self
            .inner
            .lock()
            .entries
            .iter()
            .map(|s| s.entry.clone())
            .collect();
        current.sort_by(|a, b| a.key.cmp(&b.key));
        let contexts = try_join_all(current.iter().map(|entry| (entry.load)())).await?;
        combine(contexts)
    }
}
