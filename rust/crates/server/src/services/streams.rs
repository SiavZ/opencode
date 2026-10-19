//! Purpose: service ports consumed by the streaming transports: the event source (current global
//! stream, durable Session replay, legacy instance stream), the legacy `GlobalBus`, the
//! location-scoped PTY service lookup, PTY connect tickets (`packages/core/src/pty/ticket.ts`)
//! and the WebSocket tracker joined on shutdown (`httpapi/websocket-tracker.ts`).
//! Scope: process-level services shared by every streaming request.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- platform-api/src/pty.rs
//! `-- server/src/services/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures_util::stream::BoxStream
//! |-- serde_json::Value
//! |-- tokio_util::sync::CancellationToken
//! |-- tokio_util::task::TaskTracker
//! |-- tokio_util::task::task_tracker::TaskTrackerToken
//! `-- uuid::Uuid::new_v4
//! Used by:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/services/mod.rs
//! |-- server/tests/sse_events.rs
//! `-- server/tests/ws_pty.rs
//! Invariants: `EventSource` is blanket-implemented for every upstream `EventStore`, so hosts pass
//! the real store. `all_bounded` overflow ends the subscriber with an error, never drops silently.
//! `durable` registers its wake before reading history (event-api contract). Tickets expire after
//! 60 s, hold at most 10_000 entries, and are consumed atomically only when the PTY ID and
//! placement match. A mismatched consume leaves the ticket in place like `Cache.invalidateWhen`.
//! The tracker refuses new sockets once closing and joins every registered socket task.
//! Contract tests: server/tests/ws_pty.rs, server/tests/sse_events.rs

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::BoxStream;
use opencode_event_api::{EventError, EventStore, EventStream};
use opencode_platform_api::pty::PtyService;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

use crate::services::PortError;

/// Object-safe view of `EventV2.Interface` used by streams.
pub trait EventSource: Send + Sync {
    /// `EventV2.allBounded(events, capacity)`.
    fn all_bounded(&self, capacity: usize) -> EventStream;
    /// `events.listen` as an unbounded stream (legacy `/event`).
    fn all(&self) -> EventStream;
    /// `events.durable({ aggregateID, after })`.
    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream;
}

impl<T: EventStore> EventSource for T {
    fn all_bounded(&self, capacity: usize) -> EventStream {
        EventStore::all_bounded(self, capacity)
    }
    fn all(&self) -> EventStream {
        EventStore::all(self)
    }
    fn durable(&self, aggregate_id: &str, after: Option<i64>) -> EventStream {
        EventStore::durable(self, aggregate_id, after)
    }
}

/// Legacy `GlobalBus` event: `{ directory?, project?, workspace?, payload }`, kept as JSON so the
/// legacy wire order is whatever the emitter produced.
pub type GlobalEvent = Value;

/// Legacy process-wide bus (`packages/opencode/src/bus/global.ts`). Subscription is registered
/// before the stream is returned.
pub trait GlobalBus: Send + Sync {
    fn subscribe(&self) -> BoxStream<'static, GlobalEvent>;
}

/// Location-scoped PTY service lookup (`LocationServiceMap.get(ref)` providing `Pty.Service`).
#[async_trait::async_trait]
pub trait PtyProvider: Send + Sync {
    async fn pty(
        &self,
        directory: &str,
        workspace_id: Option<&str>,
    ) -> Result<Arc<dyn PtyService>, PortError>;
}

/// `PtyTicket.Scope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketScope {
    pub pty_id: String,
    pub directory: Option<String>,
    pub workspace_id: Option<String>,
}

/// `PtyTicket.ConnectToken`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConnectToken {
    pub ticket: String,
    pub expires_in: u64,
}

/// `PtyTicket.Interface`.
pub trait PtyTickets: Send + Sync {
    fn issue(&self, scope: TicketScope) -> ConnectToken;
    fn consume(&self, ticket: &str, scope: &TicketScope) -> bool;
}

/// In-process ticket cache (`PtyTicket.make`): TTL, capacity, atomic match-and-remove.
pub struct TicketStore {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, (TicketScope, Instant)>>,
}

impl TicketStore {
    pub const DEFAULT_TTL: Duration = Duration::from_secs(60);
    pub const CAPACITY: usize = 10_000;

    pub fn new(ttl: Duration) -> Self {
        TicketStore {
            ttl,
            capacity: Self::CAPACITY,
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for TicketStore {
    fn default() -> Self {
        TicketStore::new(Self::DEFAULT_TTL)
    }
}

impl PtyTickets for TicketStore {
    fn issue(&self, scope: TicketScope) -> ConnectToken {
        let ticket = uuid::Uuid::new_v4().to_string();
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, (_, at)| now.duration_since(*at) < self.ttl);
        if entries.len() >= self.capacity {
            // Effect `Cache` evicts the oldest entry when full.
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(ticket.clone(), (scope, now));
        ConnectToken {
            ticket,
            expires_in: self.ttl.as_secs_f64().round().max(1.0) as u64,
        }
    }

    fn consume(&self, ticket: &str, scope: &TicketScope) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let Some((stored, at)) = entries.get(ticket) else {
            return false;
        };
        if at.elapsed() >= self.ttl {
            entries.remove(ticket);
            return false;
        }
        if stored != scope {
            return false;
        }
        entries.remove(ticket);
        true
    }
}

/// `WebSocketTracker`: owns every accepted WebSocket task so shutdown can close and join them.
#[derive(Default)]
pub struct SocketTracker {
    closing: CancellationToken,
    tasks: TaskTracker,
}

impl SocketTracker {
    /// `register`: a token held by the socket task, `None` once closing (the caller closes the
    /// socket with 1001 `server closing`).
    pub fn register(&self) -> Option<(TaskTrackerToken, CancellationToken)> {
        if self.closing.is_cancelled() {
            return None;
        }
        Some((self.tasks.token(), self.closing.clone()))
    }

    /// `closeAll`: every socket receives 1001 `server closing`, then all socket tasks are joined.
    pub async fn close_all(&self) {
        self.closing.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    pub fn active(&self) -> usize {
        self.tasks.len()
    }
}

/// Streaming services bundle, part of `Services`.
#[derive(Clone)]
pub struct StreamServices {
    pub events: Option<Arc<dyn EventSource>>,
    pub global: Option<Arc<dyn GlobalBus>>,
    pub pty: Option<Arc<dyn PtyProvider>>,
    pub tickets: Arc<dyn PtyTickets>,
    pub sockets: Arc<SocketTracker>,
}

impl Default for StreamServices {
    fn default() -> Self {
        StreamServices {
            events: None,
            global: None,
            pty: None,
            tickets: Arc::new(TicketStore::default()),
            sockets: Arc::new(SocketTracker::default()),
        }
    }
}

impl std::fmt::Debug for StreamServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamServices")
            .field("events", &self.events.is_some())
            .field("global", &self.global.is_some())
            .field("pty", &self.pty.is_some())
            .finish()
    }
}

/// Convenience re-export for adapters.
pub type EventResult = Result<opencode_event_api::Payload, EventError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(id: &str) -> TicketScope {
        TicketScope {
            pty_id: id.into(),
            directory: Some("/d".into()),
            workspace_id: None,
        }
    }

    #[test]
    fn ticket_is_single_use_and_scoped() {
        let store = TicketStore::default();
        let token = store.issue(scope("pty_1"));
        assert_eq!(token.expires_in, 60);
        assert!(!store.consume(&token.ticket, &scope("pty_2")));
        assert!(store.consume(&token.ticket, &scope("pty_1")));
        assert!(!store.consume(&token.ticket, &scope("pty_1")));
    }

    #[test]
    fn ticket_expires() {
        let store = TicketStore::new(Duration::from_millis(1));
        let token = store.issue(scope("pty_1"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(!store.consume(&token.ticket, &scope("pty_1")));
    }
}
