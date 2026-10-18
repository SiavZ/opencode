//! Purpose: lifetime of one location bundle: cancellation token, owned tasks and finalizers run in
//! reverse registration order (the scope a `Layer` build acquires into in location-services.ts).
//! Scope: `LocationScope` value and its one-shot close.
//! Uses:
//! rust/crates/
//! `-- locations/src/registry.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- tokio::sync::OnceCell
//! |-- tokio_util::sync::CancellationToken
//! `-- tokio_util::task::TaskTracker
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! `-- locations/src/registry.rs
//! Invariants: close runs once; it cancels the token, joins every owned task, runs the bundle
//! teardown, then the finalizers last-registered first. Concurrent closers all wait for completion.
//! Contract tests: locations/tests/registry.rs

use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::BoxFuture;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::registry::LocationServices;

type Finalizer = BoxFuture<'static, ()>;

struct ScopeInner {
    token: CancellationToken,
    tasks: TaskTracker,
    finalizers: Mutex<Vec<(String, Finalizer)>>,
    closed: OnceCell<()>,
}

/// Lifetime of one location bundle: cancellation token, owned tasks and ordered finalizers.
#[derive(Clone)]
pub struct LocationScope {
    inner: Arc<ScopeInner>,
}

impl std::fmt::Debug for LocationScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocationScope")
            .field("cancelled", &self.inner.token.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl LocationScope {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                token: CancellationToken::new(),
                tasks: TaskTracker::new(),
                finalizers: Mutex::new(Vec::new()),
                closed: OnceCell::new(),
            }),
        }
    }

    /// Cancelled when teardown starts.
    pub fn token(&self) -> CancellationToken {
        self.inner.token.clone()
    }

    /// Spawn a task owned by the location. Teardown cancels `token()` and joins it.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.inner.tasks.spawn(future)
    }

    /// Register a finalizer. Finalizers run in reverse registration order.
    pub fn add_finalizer(
        &self,
        name: impl Into<String>,
        finalizer: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        lock(&self.inner.finalizers).push((name.into(), Box::pin(finalizer)));
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.initialized()
    }

    pub(crate) async fn close(&self, services: Option<&dyn LocationServices>) {
        self.inner
            .closed
            .get_or_init(|| async {
                self.inner.token.cancel();
                self.inner.tasks.close();
                self.inner.tasks.wait().await;
                if let Some(services) = services {
                    services.teardown().await;
                }
                let finalizers = std::mem::take(&mut *lock(&self.inner.finalizers));
                for (_, finalizer) in finalizers.into_iter().rev() {
                    finalizer.await;
                }
            })
            .await;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
