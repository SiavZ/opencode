//! Purpose: typed event errors (`EventV2.InvalidDurableEvent`, `EventV2.SubscriberOverflow`) plus storage
//! and projector failures.
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! `-- event-api/src/store.rs
//! Invariants: messages for InvalidDurableEvent match packages/core/src/event.ts text exactly.
//! Contract tests: event-store/tests/commit.rs

/// Boxed error from a projector or commit hook.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    /// `EventV2.InvalidDurableEvent`: unknown type, aggregate mismatch, divergence, sequence gap,
    /// owner mismatch or duplicate ID. Nothing was committed.
    #[error("{message}")]
    InvalidDurableEvent { event_type: String, message: String },
    /// `EventV2.SubscriberOverflow`: a bounded subscriber fell behind.
    #[error("subscriber overflow (capacity {capacity})")]
    SubscriberOverflow { capacity: usize },
    /// A projector or local commit hook failed. The event transaction rolled back.
    /// Downcast `source` to recover the caller's typed error.
    #[error("projector for {event_type} failed: {source}")]
    Projector {
        event_type: String,
        #[source]
        source: BoxError,
    },
    /// Data did not match the definition codec.
    #[error("invalid event data for {event_type}: {message}")]
    Codec { event_type: String, message: String },
    /// Storage failure (busy, constraint, closed). `retryable` mirrors the storage classification.
    #[error("event storage failed: {message}")]
    Storage {
        message: String,
        retryable: bool,
        source: Option<BoxError>,
    },
    /// The operation was cancelled before commit.
    #[error("event operation cancelled")]
    Cancelled,
    /// The store was shut down.
    #[error("event store closed")]
    Closed,
}

impl EventError {
    pub fn invalid(event_type: impl Into<String>, message: impl Into<String>) -> Self {
        EventError::InvalidDurableEvent {
            event_type: event_type.into(),
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            EventError::Storage {
                retryable: true,
                ..
            }
        )
    }

    /// Downcast a projector failure back to the caller's error type.
    pub fn projector_source<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            EventError::Projector { source, .. } => source.downcast_ref::<E>(),
            _ => None,
        }
    }
}
