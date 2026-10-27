//! Purpose: expire idle Location generations without removing a newer generation at the same key.
//! Scope: deadline rechecks for the lease-owned idle timer.
//! Uses:
//! rust/crates/
//! `-- locations/src/registry.rs
//! External:
//! |-- tokio::time::Instant
//! `-- tokio::time::sleep_until
//! Used by:
//! rust/crates/
//! `-- locations/src/registry.rs
//! Invariants: every deadline is rechecked under the map and reference locks; active leases are never evicted.
//! Contract tests: locations/tests/registry.rs

use super::{Inner, LocationServices, Slot, lock, remove_if_same};
use std::sync::Arc;
use tokio::time::{Instant, sleep_until};

pub(super) async fn idle_timer<S: LocationServices, G: Send + Sync + 'static>(
    registry: std::sync::Weak<Inner<S, G>>,
    slot: Arc<Slot<S>>,
) {
    loop {
        let deadline = {
            let refs = lock(&slot.refs);
            refs.expires_at
        };
        let Some(deadline) = deadline else {
            lock(&slot.refs).timer = false;
            return;
        };
        sleep_until(deadline).await;
        let Some(registry) = registry.upgrade() else {
            return;
        };
        let mut map = lock(&registry.map);
        let mut refs = lock(&slot.refs);
        if refs.removed || refs.count > 0 {
            refs.timer = false;
            return;
        }
        match refs.expires_at {
            Some(at) if at <= Instant::now() => {
                refs.removed = true;
                refs.timer = false;
                remove_if_same(&mut map, &slot);
            }
            _ => continue,
        }
        drop(refs);
        drop(map);
        registry.spawn_teardown(slot);
        return;
    }
}
