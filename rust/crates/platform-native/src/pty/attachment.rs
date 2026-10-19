//! Purpose: PTY attachments with retained replay, explicit live activation and bounded delivery.
//! Scope: one subscriber, automatically detached on drop.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! |-- platform-native/src/pty/mod.rs
//! `-- platform-native/src/pty/state.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::sync::Notify
//! Used by:
//! rust/crates/
//! `-- platform-native/src/pty/mod.rs
//! Invariants: replay and cursor are captured atomically with subscription; activation never duplicates replay.
//! Contract tests: platform-native/tests/pty.rs

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use opencode_platform_api::pty::{PtyAttachment, PtyError, PtyOutput};

use super::state::{State, Subscriber, lock};

pub(super) struct Attachment {
    pub state: Weak<Mutex<State>>,
    pub subscriber: Arc<Subscriber>,
    pub id: u64,
    pub replay: String,
    pub cursor: u64,
    pub active: AtomicBool,
    pub detached: AtomicBool,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyAttachment")
            .field("cursor", &self.cursor)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PtyAttachment for Attachment {
    fn replay(&self) -> &str {
        &self.replay
    }
    fn cursor(&self) -> u64 {
        self.cursor
    }
    fn write(&self, data: &str) -> Result<(), PtyError> {
        if self.detached.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(state) = self.state.upgrade() {
            super::write(&mut lock(&state), data)?;
        }
        Ok(())
    }
    fn activate(&self) {
        self.active.store(true, Ordering::Release);
        self.subscriber.ready.notify_one();
    }
    async fn next(&self) -> Option<PtyOutput> {
        loop {
            let ready = self.subscriber.ready.notified();
            if self.detached.load(Ordering::Acquire) {
                return None;
            }
            if self.active.load(Ordering::Acquire) {
                let mut queue = lock(&self.subscriber.queue);
                if let Some(output) = queue.0.pop_front() {
                    if let PtyOutput::Data(text) = &output {
                        queue.1 -= text.len();
                    }
                    return Some(output);
                }
                if queue.2 {
                    return None;
                }
            }
            ready.await;
        }
    }
    fn detach(&self) {
        self.detached.store(true, Ordering::Release);
        if let Some(state) = self.state.upgrade() {
            lock(&state).subscribers.remove(&self.id);
        }
        let mut queue = lock(&self.subscriber.queue);
        queue.0.clear();
        queue.1 = 0;
        queue.2 = true;
        drop(queue);
        self.subscriber.ready.notify_waiters();
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.detach();
    }
}
