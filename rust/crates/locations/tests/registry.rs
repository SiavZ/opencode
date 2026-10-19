//! Purpose: conformance tests for `LocationRegistry`: lazy single build per Ref under concurrency,
//! 60-minute idle eviction on a paused tokio clock, reverse-order joined teardown, global sharing,
//! invalidation during active work, failed builds and implicit-local keys.
//! Scope: live tests of the real registry with a recording test bundle (no fixtures).
//! Uses:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! `-- schema/src/ids.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::FutureExt
//! |-- futures::future::BoxFuture
//! |-- futures::future::join_all
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the baseline behavior (LayerMap/RcMap) it pins.
//! Contract tests: locations/tests/registry.rs

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use opencode_locations::ports::location_ref;
use opencode_locations::{
    BuildContext, LocationError, LocationRegistry, LocationServices, RegistryOptions,
};
use opencode_schema::ids::WorkspaceID;

type Log = Arc<Mutex<Vec<String>>>;

struct Global {
    builds: AtomicUsize,
}

struct Bundle {
    name: String,
    global: Arc<Global>,
    log: Log,
}

#[async_trait::async_trait]
impl LocationServices for Bundle {
    async fn teardown(&self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("teardown {}", self.name));
    }
}

fn factory(
    log: Log,
    builds: Arc<AtomicUsize>,
    delay: Duration,
) -> impl Fn(BuildContext<Global>) -> BoxFuture<'static, Result<Bundle, LocationError>>
+ Send
+ Sync
+ 'static {
    move |context: BuildContext<Global>| {
        let log = log.clone();
        let builds = builds.clone();
        async move {
            builds.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            let name = context.location.directory.as_str().to_owned();
            // Services are constructed in order a, b, c; finalizers must run c, b, a.
            for service in ["a", "b", "c"] {
                let log = log.clone();
                let label = format!("finalize {name} {service}");
                context.scope.add_finalizer(service, async move {
                    log.lock().unwrap().push(label);
                });
            }
            Ok(Bundle {
                name,
                global: context.global.clone(),
                log,
            })
        }
        .boxed()
    }
}

fn registry(
    ttl: Duration,
    delay: Duration,
) -> (LocationRegistry<Bundle, Global>, Log, Arc<AtomicUsize>) {
    let log: Log = Arc::default();
    let builds = Arc::new(AtomicUsize::new(0));
    let global = Arc::new(Global {
        builds: AtomicUsize::new(1),
    });
    let registry = LocationRegistry::new(
        global,
        factory(log.clone(), builds.clone(), delay),
        RegistryOptions { idle_ttl: ttl },
    );
    (registry, log, builds)
}

#[tokio::test(start_paused = true)]
async fn concurrent_get_for_same_ref_builds_once() {
    let (registry, _, builds) = registry(Duration::from_secs(3600), Duration::from_millis(50));
    let location = location_ref("/repo", None);
    let leases = futures::future::join_all((0..16).map(|_| registry.get(&location))).await;
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "RcMap acquires once per key"
    );
    let first = leases[0].as_ref().unwrap();
    for lease in &leases {
        assert!(Arc::ptr_eq(
            lease.as_ref().unwrap().services(),
            first.services()
        ));
    }
    assert_eq!(registry.build_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn global_services_are_shared_not_rebuilt() {
    let (registry, _, builds) = registry(Duration::from_secs(3600), Duration::ZERO);
    let a = registry.get(&location_ref("/a", None)).await.unwrap();
    let b = registry.get(&location_ref("/b", None)).await.unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 2, "one bundle per location");
    assert!(
        Arc::ptr_eq(&a.global, &b.global),
        "global-tagged services are hoisted"
    );
    assert!(Arc::ptr_eq(&a.global, registry.global()));
    assert_eq!(registry.global().builds.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn workspace_id_keys_separately_from_implicit_local() {
    let (registry, _, builds) = registry(Duration::from_secs(3600), Duration::ZERO);
    let workspace = WorkspaceID::new("wrk_1").unwrap();
    let _local = registry.get(&location_ref("/repo", None)).await.unwrap();
    let _remote = registry
        .get(&location_ref("/repo", Some(workspace.clone())))
        .await
        .unwrap();
    let _again = registry
        .get(&location_ref("/repo", Some(workspace)))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert_eq!(registry.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn idle_eviction_after_sixty_minutes_with_reverse_teardown() {
    let (registry, log, builds) = registry(opencode_locations::DEFAULT_IDLE_TTL, Duration::ZERO);
    let location = location_ref("/repo", None);
    drop(registry.get(&location).await.unwrap());

    tokio::time::sleep(Duration::from_secs(59 * 60)).await;
    assert!(
        registry.contains(&location),
        "still cached before 60 minutes"
    );
    assert!(log.lock().unwrap().is_empty());

    // Reuse inside the window resets the idle clock (RcMap get clears expiresAt).
    drop(registry.get(&location).await.unwrap());
    tokio::time::sleep(Duration::from_secs(59 * 60)).await;
    assert!(registry.contains(&location));
    assert_eq!(builds.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    registry.wait_teardowns().await;
    assert!(
        !registry.contains(&location),
        "evicted after 60 idle minutes"
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "teardown /repo",
            "finalize /repo c",
            "finalize /repo b",
            "finalize /repo a",
        ],
        "bundle teardown, then finalizers in reverse construction order"
    );

    drop(registry.get(&location).await.unwrap());
    assert_eq!(builds.load(Ordering::SeqCst), 2, "next get rebuilds");
}

#[tokio::test(start_paused = true)]
async fn leased_bundle_is_never_evicted() {
    let (registry, log, _) = registry(Duration::from_secs(60), Duration::ZERO);
    let location = location_ref("/repo", None);
    let lease = registry.get(&location).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3 * 3600)).await;
    assert!(registry.contains(&location));
    assert!(log.lock().unwrap().is_empty());
    drop(lease);
    tokio::time::sleep(Duration::from_secs(61)).await;
    registry.wait_teardowns().await;
    assert!(!registry.contains(&location));
}

#[tokio::test(start_paused = true)]
async fn invalidate_during_active_work_defers_teardown_until_tasks_and_leases_finish() {
    let (registry, log, builds) = registry(Duration::from_secs(3600), Duration::ZERO);
    let location = location_ref("/repo", None);
    let lease = registry.get(&location).await.unwrap();

    // An owned task doing work: it must observe cancellation and be joined before teardown.
    let token = lease.scope().token();
    let task_log = log.clone();
    lease.scope().spawn(async move {
        token.cancelled().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        task_log.lock().unwrap().push("task finished".into());
    });

    registry.invalidate(&location).await;
    assert!(
        !registry.contains(&location),
        "invalidate forgets the key immediately"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "active lease keeps the bundle alive"
    );
    assert!(!lease.scope().is_closed());

    // A new get builds a fresh bundle while the old one is still in use.
    let fresh = registry.get(&location).await.unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert!(!Arc::ptr_eq(fresh.services(), lease.services()));

    let old_scope = lease.scope().clone();
    drop(lease);
    registry.wait_teardowns().await;
    assert!(old_scope.is_closed());
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "task finished",
            "teardown /repo",
            "finalize /repo c",
            "finalize /repo b",
            "finalize /repo a",
        ],
        "tasks are cancelled and joined before services and finalizers"
    );
    assert!(
        registry.contains(&location),
        "the fresh bundle is unaffected"
    );
}

#[tokio::test(start_paused = true)]
async fn invalidate_idle_bundle_tears_down_before_returning() {
    let (registry, log, _) = registry(Duration::from_secs(3600), Duration::ZERO);
    let location = location_ref("/repo", None);
    drop(registry.get(&location).await.unwrap());
    registry.invalidate(&location).await;
    assert_eq!(log.lock().unwrap().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn shutdown_tears_down_everything_and_rejects_get() {
    let (registry, log, _) = registry(Duration::from_secs(3600), Duration::ZERO);
    let a = registry.get(&location_ref("/a", None)).await.unwrap();
    drop(registry.get(&location_ref("/b", None)).await.unwrap());
    registry.shutdown().await;
    assert!(
        a.scope().is_closed(),
        "scope close releases leased entries too"
    );
    assert_eq!(log.lock().unwrap().len(), 8);
    assert!(matches!(
        registry.get(&location_ref("/a", None)).await,
        Err(LocationError::Closed)
    ));
    drop(a);
    registry.wait_teardowns().await;
    assert_eq!(log.lock().unwrap().len(), 8, "teardown runs once");
}

#[tokio::test(start_paused = true)]
async fn failed_build_caches_nothing_and_releases_partial_finalizers() {
    let log: Log = Arc::default();
    let attempts = Arc::new(AtomicUsize::new(0));
    let factory_log = log.clone();
    let factory_attempts = attempts.clone();
    let registry: LocationRegistry<Bundle, ()> = LocationRegistry::new(
        Arc::new(()),
        move |context: BuildContext<()>| {
            let log = factory_log.clone();
            let attempts = factory_attempts.clone();
            async move {
                let log2 = log.clone();
                context.scope.add_finalizer("partial", async move {
                    log2.lock().unwrap().push("finalize partial".into());
                });
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(LocationError::Build {
                        message: "boom".into(),
                    });
                }
                Ok(Bundle {
                    name: "ok".into(),
                    global: Arc::new(Global {
                        builds: AtomicUsize::new(0),
                    }),
                    log,
                })
            }
            .boxed()
        },
        RegistryOptions::default(),
    );
    let location = location_ref("/repo", None);
    let error = registry.get(&location).await.unwrap_err();
    assert!(matches!(error, LocationError::Build { .. }));
    assert_eq!(*log.lock().unwrap(), vec!["finalize partial"]);
    assert!(!registry.contains(&location));
    assert!(registry.get(&location).await.is_ok(), "next get retries");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
