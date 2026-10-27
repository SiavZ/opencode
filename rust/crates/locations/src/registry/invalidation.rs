//! Purpose: invalidate all cached Locations without closing their registry to future acquisitions.
//! Scope: generation-preserving runtime configuration reload and global dispose.
//! Uses:
//! rust/crates/
//! `-- locations/src/registry.rs
//! External:
//! `-- futures::future::join_all
//! Used by:
//! rust/crates/
//! `-- locations/src/registry.rs
//! Invariants: the cache is drained atomically; active old leases retain their own generation until released.
//! Contract tests: locations/tests/registry.rs

use super::{LocationRegistry, LocationServices, lock};

impl<S: LocationServices, G: Send + Sync + 'static> LocationRegistry<S, G> {
    /// Forget all current entries while keeping the registry open, just like per-entry invalidation.
    /// Existing leases finish on their original services; later acquisitions build fresh bundles.
    pub async fn invalidate_all(&self) {
        let idle: Vec<_> = {
            let mut map = lock(&self.inner.map);
            map.drain()
                .filter_map(|(_, slot)| {
                    let mut refs = lock(&slot.refs);
                    refs.removed = true;
                    (refs.count == 0).then(|| slot.clone())
                })
                .collect()
        };
        futures::future::join_all(idle.into_iter().map(|slot| async move {
            slot.teardown().await;
        }))
        .await;
    }
}
