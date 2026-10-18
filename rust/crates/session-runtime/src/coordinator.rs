//! Purpose: process-global keyed run coordinator (`SessionRunCoordinator`). Serializes drains per
//! key while letting different keys run concurrently. Does not know about Sessions or runners.
//! Scope: native, tokio. Owned tasks in a `TaskTracker`, cancellation via `CancellationToken`.
//! Uses:
//! `-- none
//! External:
//! |-- futures::future::{BoxFuture, FutureExt}
//! |-- tokio::sync::watch
//! `-- tokio_util::{sync::CancellationToken, task::TaskTracker}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/tests/coordinator.rs
//! Invariants: at most one owner task per key. `run` starts (force=true) or joins. `wake` starts
//! (force=false) or coalesces into one successor. `interrupt` clears the pending wake, cancels the
//! owner and waits for its settlement; idle interrupt is a no-op. Joined waiters dropping never
//! cancel the drain. The map lock is never held across an await. Drains observe the token passed
//! to them and may run async cleanup before returning; a drain whose entry was stopped settles
//! as `RunError::Interrupted` regardless of its own result.
//! Contract tests: session-runtime/tests/coordinator.rs

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::{BoxFuture, FutureExt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Drain callback: `(key, force, cancellation)`. `force` is true for explicit runs and false for
/// advisory wakes and coalesced successors.
pub type Drain<K, E> =
    Arc<dyn Fn(K, bool, CancellationToken) -> BoxFuture<'static, Result<(), E>> + Send + Sync>;

/// Outcome observed by `run` callers. Mirrors Effect's fail / interrupt / die exits.
#[derive(Debug)]
pub enum RunError<E> {
    /// The drain returned an error (`Cause.hasFails`). Shared between all joined waiters.
    Failed(Arc<E>),
    /// The drain was interrupted or the coordinator shut down (`Cause.hasInterruptsOnly`).
    Interrupted,
    /// The drain panicked (`Cause.hasDies`).
    Panicked(Arc<str>),
}

impl<E> Clone for RunError<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Failed(error) => Self::Failed(error.clone()),
            Self::Interrupted => Self::Interrupted,
            Self::Panicked(message) => Self::Panicked(message.clone()),
        }
    }
}

impl<E: fmt::Display> fmt::Display for RunError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => error.fmt(f),
            Self::Interrupted => f.write_str("execution interrupted"),
            Self::Panicked(message) => write!(f, "execution panicked: {message}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RunError<E> {}

type Outcome<E> = Result<(), RunError<E>>;

struct Entry<E> {
    id: u64,
    done: watch::Sender<Option<Outcome<E>>>,
    cancel: CancellationToken,
    pending_wake: bool,
    stopping: bool,
}

struct State<K, E> {
    active: HashMap<K, Entry<E>>,
    next_id: u64,
    closed: bool,
}

struct Inner<K, E> {
    state: Mutex<State<K, E>>,
    drain: Drain<K, E>,
    tracker: TaskTracker,
    root: CancellationToken,
    runtime: tokio::runtime::Handle,
}

/// Serializes execution for each key while allowing different keys to run concurrently.
pub struct RunCoordinator<K, E> {
    inner: Arc<Inner<K, E>>,
}

impl<K, E> Clone for RunCoordinator<K, E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K, E> RunCoordinator<K, E>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    /// Creates a coordinator bound to the current tokio runtime. Panics outside a runtime.
    pub fn new<F>(drain: F) -> Self
    where
        F: Fn(K, bool, CancellationToken) -> BoxFuture<'static, Result<(), E>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: HashMap::new(),
                    next_id: 0,
                    closed: false,
                }),
                drain: Arc::new(drain),
                tracker: TaskTracker::new(),
                root: CancellationToken::new(),
                runtime: tokio::runtime::Handle::current(),
            }),
        }
    }

    /// Snapshots keys with an execution owned by this coordinator.
    pub fn active(&self) -> HashSet<K> {
        self.inner.lock().active.keys().cloned().collect()
    }

    /// Starts execution while idle (force=true) or joins the active execution.
    pub async fn run(&self, key: K) -> Outcome<E> {
        loop {
            let (mut done, stopping) = {
                let mut state = self.inner.lock();
                if state.closed {
                    return Err(RunError::Interrupted);
                }
                match state.active.get(&key) {
                    Some(entry) => (entry.done.subscribe(), entry.stopping),
                    None => {
                        let receiver = self.inner.start_entry(&mut state, key.clone(), true);
                        (receiver, false)
                    }
                }
            };
            let outcome = await_done(&mut done).await;
            // A stopping entry finished its cleanup; start (or join a successor) afresh.
            if !stopping {
                return outcome;
            }
        }
    }

    /// Registers one coalesced follow-up after newly recorded work. Starts force=false when idle.
    pub fn wake(&self, key: K) {
        let mut state = self.inner.lock();
        if state.closed {
            return;
        }
        if let Some(entry) = state.active.get_mut(&key) {
            entry.pending_wake = true;
            return;
        }
        drop(self.inner.start_entry(&mut state, key, false));
    }

    /// Stops active execution and waits for its cleanup. Idle interruption is a no-op.
    pub async fn interrupt(&self, key: K) {
        let mut done = {
            let mut state = self.inner.lock();
            let Some(entry) = state.active.get_mut(&key) else {
                return;
            };
            entry.stopping = true;
            entry.pending_wake = false;
            entry.cancel.cancel();
            entry.done.subscribe()
        };
        let _ = await_done(&mut done).await;
    }

    /// Cancels every owned execution, refuses new work and waits for all owner tasks to settle.
    pub async fn shutdown(&self) {
        {
            let mut state = self.inner.lock();
            state.closed = true;
            for entry in state.active.values_mut() {
                entry.stopping = true;
                entry.pending_wake = false;
            }
        }
        self.inner.root.cancel();
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
    }
}

impl<K, E> Inner<K, E>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    fn lock(&self) -> MutexGuard<'_, State<K, E>> {
        // A poisoned map only means a panic happened while it was held; the data stays consistent
        // because every critical section is a few non-panicking field updates.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn start_entry(
        self: &Arc<Self>,
        state: &mut State<K, E>,
        key: K,
        force: bool,
    ) -> watch::Receiver<Option<Outcome<E>>> {
        let id = state.next_id;
        state.next_id += 1;
        let (done, receiver) = watch::channel(None);
        let cancel = self.root.child_token();
        state.active.insert(
            key.clone(),
            Entry {
                id,
                done,
                cancel: cancel.clone(),
                pending_wake: false,
                stopping: false,
            },
        );
        let inner = self.clone();
        self.tracker
            .spawn_on(inner.own(key, id, cancel, force), &self.runtime);
        receiver
    }

    /// Owner task for one entry. Restarts the same entry for coalesced successors after success,
    /// which keeps self-waking drains iterative instead of recursive.
    async fn own(self: Arc<Self>, key: K, id: u64, cancel: CancellationToken, force: bool) {
        let mut force = force;
        loop {
            // The drain call itself sits inside the async block so a synchronous panic while
            // building the future is caught too.
            let result =
                AssertUnwindSafe(async { (self.drain)(key.clone(), force, cancel.clone()).await })
                    .catch_unwind()
                    .await;
            if !self.settle(&key, id, &cancel, result) {
                return;
            }
            force = false;
            // Yield between coalesced successors so self-waking drains cannot starve the runtime.
            tokio::task::yield_now().await;
        }
    }

    /// Settles one drain attempt under the map lock. Returns true when the same entry must run a
    /// coalesced successor (success with a pending wake and not stopping).
    fn settle(
        self: &Arc<Self>,
        key: &K,
        id: u64,
        cancel: &CancellationToken,
        result: Result<Result<(), E>, Box<dyn std::any::Any + Send>>,
    ) -> bool {
        let mut state = self.lock();
        let Some(entry) = state.active.get_mut(key).filter(|entry| entry.id == id) else {
            return false;
        };
        let outcome = match result {
            _ if entry.stopping || cancel.is_cancelled() => Err(RunError::Interrupted),
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(RunError::Failed(Arc::new(error))),
            Err(panic) => Err(RunError::Panicked(panic_message(panic))),
        };
        if outcome.is_ok() && entry.pending_wake {
            entry.pending_wake = false;
            return true;
        }
        let successor = entry.pending_wake && !state.closed;
        let Some(entry) = state.active.remove(key) else {
            return false;
        };
        if successor {
            drop(self.start_entry(&mut state, key.clone(), false));
        }
        drop(state);
        entry.done.send_replace(Some(outcome));
        false
    }
}

async fn await_done<E>(done: &mut watch::Receiver<Option<Outcome<E>>>) -> Outcome<E> {
    match done.wait_for(Option::is_some).await {
        Ok(value) => value.clone().unwrap_or(Err(RunError::Interrupted)),
        // The sender was dropped without settling (owner task aborted by runtime shutdown).
        Err(_) => Err(RunError::Interrupted),
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> Arc<str> {
    if let Some(message) = panic.downcast_ref::<&str>() {
        return Arc::from(*message);
    }
    if let Some(message) = panic.downcast_ref::<String>() {
        return Arc::from(message.as_str());
    }
    Arc::from("drain panicked")
}
