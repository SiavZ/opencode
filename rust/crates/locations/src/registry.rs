//! Purpose: Ref-keyed, lazily built, reference-counted registry of per-location service bundles
//! (port of `LayerMap.make(..., { idleTimeToLive: "60 minutes" })` in
//! packages/core/src/location-services.ts:83-112 and location-service-map.ts:7-18).
//! Scope: generic registry and leases. Location scopes live in scope.rs.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/scope.rs
//! `-- schema/src/location.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::future::BoxFuture
//! |-- tokio::runtime::Handle
//! |-- tokio::runtime::Handle::current
//! |-- tokio::select
//! |-- tokio::sync::OnceCell
//! |-- tokio::sync::watch
//! |-- tokio::time::Instant
//! |-- tokio::time::sleep_until
//! |-- tokio_util::sync::CancellationToken
//! |-- tokio_util::task::TaskTracker
//! |-- tracing::info
//! |-- watch::Sender
//! `-- watch::channel
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! `-- locations/src/scope.rs
//! Invariants: one build per live Ref even under concurrent `get`; global services (`G`) are built by
//! the owner once and shared by every bundle; a bundle is torn down only when no lease is live
//! (explicit `invalidate` defers until the last lease drops, like `RcMap.invalidate`), or on
//! `shutdown`; teardown cancels and joins the location's tasks, then runs `LocationServices::teardown`,
//! then the scope finalizers in reverse registration order, and completes before eviction is
//! observable through `wait_teardowns`/`shutdown`. A failed build caches nothing and the next
//! `get` retries. Omitted `workspace_id` is implicit-local and keys
//! separately from any explicit workspace.
//! Contract tests: locations/tests/registry.rs

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use opencode_schema::location::Ref;
use tokio::sync::{OnceCell, watch};
use tokio::time::Instant;
#[path = "registry/idle.rs"]
mod idle;
use idle::idle_timer;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::error::{LocationError, Result};
pub use crate::scope::LocationScope;

#[path = "registry/invalidation.rs"]
mod invalidation;

/// `idleTimeToLive: "60 minutes"`.
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(60 * 60);

/// A per-location service bundle. `teardown` runs once, after the location's tasks are joined and
/// before the scope finalizers registered during the build.
#[async_trait]
pub trait LocationServices: Send + Sync + 'static {
    async fn teardown(&self) {}
}

/// Builds one bundle for a Ref. Finalizers registered on `scope` run in reverse order on teardown,
/// and also when the build itself fails (like a failed `Layer` acquisition releasing what it acquired).
/// Any `Fn(BuildContext<G>) -> impl Future<Output = Result<S>>` closure is a factory.
pub trait LocationFactory<S, G>: Send + Sync + 'static {
    fn build(&self, context: BuildContext<G>) -> BoxFuture<'static, Result<S>>;
}

impl<S, G, F, Fut> LocationFactory<S, G> for F
where
    F: Fn(BuildContext<G>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S>> + Send + 'static,
{
    fn build(&self, context: BuildContext<G>) -> BoxFuture<'static, Result<S>> {
        Box::pin(self(context))
    }
}

/// Input of one build.
pub struct BuildContext<G> {
    pub location: Ref,
    /// Process-wide (global-tagged) services, hoisted out of the per-location graph.
    pub global: Arc<G>,
    pub scope: LocationScope,
}

impl<G> std::fmt::Debug for BuildContext<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildContext")
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

/// Registry options.
#[derive(Debug, Clone)]
pub struct RegistryOptions {
    /// Delay between the last lease dropping and teardown. Zero tears down immediately.
    pub idle_ttl: Duration,
}

impl Default for RegistryOptions {
    fn default() -> Self {
        Self {
            idle_ttl: DEFAULT_IDLE_TTL,
        }
    }
}

type Key = (String, Option<String>);

fn key_of(location: &Ref) -> Key {
    (
        location.directory.as_str().to_owned(),
        location
            .workspace_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
    )
}

struct Built<S> {
    services: Arc<S>,
    scope: LocationScope,
}

#[derive(Default)]
struct Refs {
    count: usize,
    expires_at: Option<Instant>,
    timer: bool,
    removed: bool,
}

struct Slot<S> {
    location: Ref,
    cell: OnceCell<Built<S>>,
    refs: Mutex<Refs>,
}

impl<S: LocationServices> Slot<S> {
    async fn teardown(&self) {
        if let Some(built) = self.cell.get() {
            built.scope.close(Some(built.services.as_ref())).await;
        }
    }
}

struct Inner<S, G> {
    global: Arc<G>,
    factory: Box<dyn LocationFactory<S, G>>,
    options: RegistryOptions,
    map: Mutex<HashMap<Key, Arc<Slot<S>>>>,
    closed: Mutex<bool>,
    /// Idle timers. Cancelled by `shutdown`.
    timers: TaskTracker,
    timer_cancel: CancellationToken,
    /// Number of background teardowns in flight.
    teardowns: watch::Sender<usize>,
    runtime: tokio::runtime::Handle,
    builds: Mutex<u64>,
}

impl<S: LocationServices, G> Inner<S, G> {
    fn spawn_teardown(&self, slot: Arc<Slot<S>>) {
        self.teardowns.send_modify(|n| *n += 1);
        let counter = self.teardowns.clone();
        self.runtime.spawn(async move {
            slot.teardown().await;
            counter.send_modify(|n| *n -= 1);
        });
    }
}

/// `LocationServiceMap`: lazily builds and caches one bundle per `Location.Ref`.
pub struct LocationRegistry<S, G = ()> {
    inner: Arc<Inner<S, G>>,
}

impl<S, G> Clone for LocationRegistry<S, G> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S, G> std::fmt::Debug for LocationRegistry<S, G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocationRegistry")
            .field("live", &lock(&self.inner.map).len())
            .finish_non_exhaustive()
    }
}

impl<S: LocationServices, G: Send + Sync + 'static> LocationRegistry<S, G> {
    /// Must be called inside a tokio runtime (teardown tasks spawn on it).
    pub fn new(
        global: Arc<G>,
        factory: impl LocationFactory<S, G>,
        options: RegistryOptions,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                global,
                factory: Box::new(factory),
                options,
                map: Mutex::new(HashMap::new()),
                closed: Mutex::new(false),
                timers: TaskTracker::new(),
                timer_cancel: CancellationToken::new(),
                teardowns: watch::channel(0).0,
                runtime: tokio::runtime::Handle::current(),
                builds: Mutex::new(0),
            }),
        }
    }

    pub fn global(&self) -> &Arc<G> {
        &self.inner.global
    }

    /// Acquire the bundle for `location`, building it on first use. The lease keeps it alive.
    pub async fn get(&self, location: &Ref) -> Result<LocationLease<S, G>> {
        let slot = {
            if *lock(&self.inner.closed) {
                return Err(LocationError::Closed);
            }
            let mut map = lock(&self.inner.map);
            let slot = map
                .entry(key_of(location))
                .or_insert_with(|| {
                    Arc::new(Slot {
                        location: location.clone(),
                        cell: OnceCell::new(),
                        refs: Mutex::new(Refs::default()),
                    })
                })
                .clone();
            let mut refs = lock(&slot.refs);
            refs.count += 1;
            refs.expires_at = None;
            drop(refs);
            slot
        };
        // The lease exists before the build so a failed or cancelled build releases its count.
        let lease = LocationLease {
            registry: self.inner.clone(),
            slot: slot.clone(),
        };
        let inner = self.inner.clone();
        let building = slot.clone();
        slot.cell
            .get_or_try_init(|| async move {
                let slot = building;
                let scope = LocationScope::new();
                let built = inner
                    .factory
                    .build(BuildContext {
                        location: slot.location.clone(),
                        global: inner.global.clone(),
                        scope: scope.clone(),
                    })
                    .await;
                match built {
                    Ok(services) => {
                        *lock(&inner.builds) += 1;
                        tracing::info!(
                            directory = slot.location.directory.as_str(),
                            workspaceID = slot.location.workspace_id.as_ref().map(|id| id.as_str()),
                            "booting location services"
                        );
                        Ok(Built {
                            services: Arc::new(services),
                            scope,
                        })
                    }
                    Err(error) => {
                        scope.close(None).await;
                        Err(error)
                    }
                }
            })
            .await?;
        Ok(lease)
    }

    /// `RcMap.invalidate`: forget the bundle now. Torn down immediately (and awaited) when idle,
    /// otherwise when its last lease drops. The next `get` builds a fresh bundle.
    pub async fn invalidate(&self, location: &Ref) {
        let idle = {
            let mut map = lock(&self.inner.map);
            let Some(slot) = map.remove(&key_of(location)) else {
                return;
            };
            let mut refs = lock(&slot.refs);
            refs.removed = true;
            (refs.count == 0).then(|| slot.clone())
        };
        if let Some(slot) = idle {
            slot.teardown().await;
        }
    }

    /// True while a built bundle is cached for `location`.
    pub fn contains(&self, location: &Ref) -> bool {
        lock(&self.inner.map)
            .get(&key_of(location))
            .is_some_and(|slot| slot.cell.initialized())
    }

    /// Number of cached built bundles.
    pub fn len(&self) -> usize {
        lock(&self.inner.map)
            .values()
            .filter(|slot| slot.cell.initialized())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Successful builds so far.
    pub fn build_count(&self) -> u64 {
        *lock(&self.inner.builds)
    }

    /// Wait until no background teardown (idle expiry, deferred invalidation, last lease dropped)
    /// is in flight. Pending idle timers are not waited for.
    pub async fn wait_teardowns(&self) {
        let mut receiver = self.inner.teardowns.subscribe();
        let _ = receiver.wait_for(|n| *n == 0).await;
    }

    /// Close the registry: tear down every bundle (leased or not, like scope close of `RcMap`) and
    /// join all teardown tasks. Later `get` calls fail with `Closed`.
    pub async fn shutdown(&self) {
        *lock(&self.inner.closed) = true;
        let slots: Vec<_> = {
            let mut map = lock(&self.inner.map);
            map.drain()
                .map(|(_, slot)| {
                    lock(&slot.refs).removed = true;
                    slot
                })
                .collect()
        };
        for slot in slots {
            slot.teardown().await;
        }
        self.inner.timer_cancel.cancel();
        self.inner.timers.close();
        self.inner.timers.wait().await;
        self.wait_teardowns().await;
    }
}

/// A live use of one location bundle (`RcMap` reference). Dropping the last lease starts the idle timer.
pub struct LocationLease<S: LocationServices, G: Send + Sync + 'static = ()> {
    registry: Arc<Inner<S, G>>,
    slot: Arc<Slot<S>>,
}

impl<S: LocationServices, G: Send + Sync + 'static> std::fmt::Debug for LocationLease<S, G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocationLease")
            .field("location", &self.slot.location)
            .finish_non_exhaustive()
    }
}

impl<S: LocationServices, G: Send + Sync + 'static> LocationLease<S, G> {
    pub fn location(&self) -> &Ref {
        &self.slot.location
    }

    pub fn services(&self) -> &Arc<S> {
        &self.built().services
    }

    pub fn scope(&self) -> &LocationScope {
        &self.built().scope
    }

    fn built(&self) -> &Built<S> {
        self.slot
            .cell
            .get()
            .expect("a returned lease always has a built bundle")
    }
}

impl<S: LocationServices, G: Send + Sync + 'static> std::ops::Deref for LocationLease<S, G> {
    type Target = S;
    fn deref(&self) -> &S {
        self.services()
    }
}

impl<S: LocationServices, G: Send + Sync + 'static> Drop for LocationLease<S, G> {
    fn drop(&mut self) {
        let registry = self.registry.clone();
        let slot = self.slot.clone();
        let mut map = lock(&registry.map);
        let mut refs = lock(&slot.refs);
        refs.count -= 1;
        if refs.count > 0 {
            return;
        }
        let closed = *lock(&registry.closed);
        if refs.removed || closed || registry.options.idle_ttl.is_zero() {
            if !refs.removed {
                refs.removed = true;
                remove_if_same(&mut map, &slot);
            }
            drop(refs);
            drop(map);
            registry.spawn_teardown(slot);
            return;
        }
        refs.expires_at = Some(Instant::now() + registry.options.idle_ttl);
        if refs.timer {
            return;
        }
        refs.timer = true;
        drop(refs);
        drop(map);
        let weak = Arc::downgrade(&registry);
        let cancel = registry.timer_cancel.clone();
        registry.timers.spawn_on(
            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = idle_timer(weak, slot) => {}
                }
            },
            &registry.runtime,
        );
    }
}

fn remove_if_same<S>(map: &mut HashMap<Key, Arc<Slot<S>>>, slot: &Arc<Slot<S>>) {
    let key = key_of(&slot.location);
    if map
        .get(&key)
        .is_some_and(|current| Arc::ptr_eq(current, slot))
    {
        map.remove(&key);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
