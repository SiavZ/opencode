//! Purpose: process-local fan-out for committed and live-only events: listeners, typed and
//! wildcard subscribers, bounded subscribers and per-aggregate durable wakes.
//! Scope: in-memory, sync. No database access.
//! Uses:
//! rust/crates/
//! `-- event-api/src/lib.rs
//! External:
//! |-- futures::stream
//! |-- tokio::sync::{Notify, mpsc}
//! `-- tracing
//! Used by:
//! rust/crates/
//! |-- event-store/src/read.rs
//! `-- event-store/src/store.rs
//! Invariants: listeners run in registration order before stream subscribers (`notify`). Durable
//! notifications isolate listener errors and panics; live-only notifications propagate the first
//! listener error. Locks are only held to snapshot or prune, never while calling out. Closed
//! receivers are pruned on the next publish. Durable wakes are sliding(1): `Notify::notify_one`
//! keeps one permit when nobody is waiting.
//! Contract tests: event-store/tests/streams.rs

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::StreamExt;
use opencode_event_api::{BoxError, EventError, EventStream, Listener, Payload};
use tokio::sync::{Notify, mpsc};

/// Bounded subscriber registered by `all_bounded`, delivered like a listener.
struct BoundedSink {
    sender: mpsc::Sender<Payload>,
    overflowed: Arc<AtomicBool>,
}

enum Observer {
    Listener(Listener),
    Bounded(BoundedSink),
}

#[derive(Default)]
struct State {
    observers: Vec<(u64, Observer)>,
    typed: HashMap<String, Vec<mpsc::UnboundedSender<Payload>>>,
    all: Vec<mpsc::UnboundedSender<Payload>>,
    durable: HashMap<String, Vec<(u64, Arc<Notify>)>>,
}

#[derive(Default)]
pub(crate) struct Hub {
    state: Mutex<State>,
    next_id: AtomicU64,
}

enum Target {
    Listener(Listener),
    Bounded(u64, mpsc::Sender<Payload>, Arc<AtomicBool>),
}

impl Hub {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Critical sections only push, retain or clone; a poisoned map is still consistent.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn listen(self: &Arc<Self>, listener: Listener) -> Box<dyn FnOnce() + Send> {
        let id = self.id();
        self.lock()
            .observers
            .push((id, Observer::Listener(listener)));
        let hub = Arc::downgrade(self);
        Box::new(move || {
            if let Some(hub) = hub.upgrade() {
                hub.lock().observers.retain(|(entry, _)| *entry != id);
            }
        })
    }

    pub(crate) fn subscribe(&self, event_type: &str) -> EventStream {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.lock()
            .typed
            .entry(event_type.to_owned())
            .or_default()
            .push(sender);
        unbounded_stream(receiver)
    }

    pub(crate) fn all(&self) -> EventStream {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.lock().all.push(sender);
        unbounded_stream(receiver)
    }

    /// `EventV2.allBounded`: a dropping queue registered as a listener. Overflow removes the
    /// subscriber; the stream drains what it already buffered, then ends with `SubscriberOverflow`.
    pub(crate) fn all_bounded(&self, capacity: usize) -> EventStream {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        let overflowed = Arc::new(AtomicBool::new(false));
        let id = self.id();
        self.lock().observers.push((
            id,
            Observer::Bounded(BoundedSink {
                sender,
                overflowed: overflowed.clone(),
            }),
        ));
        futures::stream::unfold(
            (receiver, overflowed, false),
            move |(mut receiver, overflowed, ended)| async move {
                if ended {
                    return None;
                }
                match receiver.recv().await {
                    Some(event) => Some((Ok(event), (receiver, overflowed, false))),
                    None if overflowed.load(Ordering::Acquire) => Some((
                        Err(EventError::SubscriberOverflow { capacity }),
                        (receiver, overflowed, true),
                    )),
                    None => None,
                }
            },
        )
        .boxed()
    }

    /// Registers a durable wake for an aggregate. Dropping the guard unregisters it.
    pub(crate) fn durable_wake(self: &Arc<Self>, aggregate_id: &str) -> WakeGuard {
        let id = self.id();
        let notify = Arc::new(Notify::new());
        self.lock()
            .durable
            .entry(aggregate_id.to_owned())
            .or_default()
            .push((id, notify.clone()));
        WakeGuard {
            hub: Arc::downgrade(self),
            aggregate_id: aggregate_id.to_owned(),
            id,
            notify,
        }
    }

    /// Wakes every durable stream of an aggregate. Called only after the commit succeeded.
    pub(crate) fn wake(&self, aggregate_id: &str) {
        let wakes: Vec<_> = self
            .lock()
            .durable
            .get(aggregate_id)
            .map(|wakes| wakes.iter().map(|(_, notify)| notify.clone()).collect())
            .unwrap_or_default();
        wakes.iter().for_each(|notify| notify.notify_one());
    }

    /// `notify(event, isolateListeners)`.
    pub(crate) fn notify(&self, event: &Payload, isolate: bool) -> Result<(), BoxError> {
        let targets: Vec<Target> = self
            .lock()
            .observers
            .iter()
            .map(|(id, observer)| match observer {
                Observer::Listener(listener) => Target::Listener(listener.clone()),
                Observer::Bounded(sink) => {
                    Target::Bounded(*id, sink.sender.clone(), sink.overflowed.clone())
                }
            })
            .collect();
        let mut dead = Vec::new();
        for target in targets {
            match target {
                Target::Listener(listener) if isolate => observe(&listener, event),
                Target::Listener(listener) => listener(event)?,
                Target::Bounded(id, sender, overflowed) => match sender.try_send(event.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        overflowed.store(true, Ordering::Release);
                        dead.push(id);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => dead.push(id),
                },
            }
        }
        let mut state = self.lock();
        if !dead.is_empty() {
            // Dropping the stored sender lets an overflowed stream observe the end of its queue.
            state.observers.retain(|(id, _)| !dead.contains(id));
        }
        if let Some(senders) = state.typed.get_mut(&event.event_type) {
            senders.retain(|sender| sender.send(event.clone()).is_ok());
        }
        state
            .all
            .retain(|sender| sender.send(event.clone()).is_ok());
        Ok(())
    }
}

/// `observe`: log and swallow listener failures (including panics) after a durable commit.
fn observe(listener: &Listener, event: &Payload) {
    match catch_unwind(AssertUnwindSafe(|| listener(event))) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(
            event_id = %event.id,
            event_type = %event.event_type,
            %error,
            "Event listener failed"
        ),
        Err(_) => tracing::error!(
            event_id = %event.id,
            event_type = %event.event_type,
            "Event listener failed: panicked"
        ),
    }
}

fn unbounded_stream(receiver: mpsc::UnboundedReceiver<Payload>) -> EventStream {
    futures::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (Ok(event), receiver))
    })
    .boxed()
}

/// Registration of one durable stream. Unregisters on drop.
pub(crate) struct WakeGuard {
    hub: std::sync::Weak<Hub>,
    aggregate_id: String,
    id: u64,
    pub(crate) notify: Arc<Notify>,
}

impl Drop for WakeGuard {
    fn drop(&mut self) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        let mut state = hub.lock();
        if let Some(wakes) = state.durable.get_mut(&self.aggregate_id) {
            wakes.retain(|(id, _)| *id != self.id);
            if wakes.is_empty() {
                state.durable.remove(&self.aggregate_id);
            }
        }
    }
}

#[cfg(test)]
impl Hub {
    pub(crate) fn durable_registrations(&self, aggregate_id: &str) -> usize {
        self.lock().durable.get(aggregate_id).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_wakes_unregister_on_drop_and_keep_one_permit() {
        let hub = Arc::new(Hub::default());
        let first = hub.durable_wake("agg");
        let second = hub.durable_wake("agg");
        assert_eq!(hub.durable_registrations("agg"), 2);
        hub.wake("agg");
        hub.wake("agg");
        // sliding(1): two wakes before anyone waits leave exactly one permit.
        assert!(first.notify.notified().now_or_never().is_some());
        assert!(first.notify.notified().now_or_never().is_none());
        drop(first);
        assert_eq!(hub.durable_registrations("agg"), 1);
        drop(second);
        assert_eq!(hub.durable_registrations("agg"), 0);
        hub.wake("agg");
    }

    #[test]
    fn listener_unsubscribe_is_idempotent_after_hub_drop() {
        let hub = Arc::new(Hub::default());
        let unsubscribe = hub.listen(Arc::new(|_| Ok(())));
        drop(hub);
        unsubscribe();
    }

    use futures::FutureExt;
}
