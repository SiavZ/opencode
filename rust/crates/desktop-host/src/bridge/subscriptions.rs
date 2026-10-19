//! Purpose: generic per-sender event subscription registry with bounded delivery channels.
//! Scope: manages subscriptions keyed by (sender_id, topic), handles replacement, idempotent disposal, and sender teardown.
//! Uses:
//! `-- none
//! External:
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- mpsc::error::TrySendError::Closed
//! |-- mpsc::error::TrySendError::Full
//! `-- tokio::sync::mpsc
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/bridge.rs
//! `-- desktop-host/tests/bridge_subscriptions.rs
//! Invariants: re-subscribing replaces previous, disposal is idempotent, window destruction disposes all sender subscriptions.
//! Contract tests: desktop-host/tests/bridge_subscriptions.rs

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub type SenderId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowBehavior {
    DropOldest,
    DropNewest,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionConfig {
    pub capacity: usize,
    pub overflow: OverflowBehavior,
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            capacity: 100,
            overflow: OverflowBehavior::DropNewest,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendResult {
    Sent,
    Dropped,
    Closed,
    NoSuchSubscription,
}

pub struct SubscriptionHandle<K: Eq + Hash + Send + Sync + 'static> {
    cleanup: Box<dyn Fn() + Send + Sync>,
    disposed: Mutex<bool>,
    _marker: std::marker::PhantomData<K>,
}

impl<K: Eq + Hash + Send + Sync + 'static> SubscriptionHandle<K> {
    pub fn dispose(&self) {
        let mut disposed = self.disposed.lock().unwrap();
        if *disposed {
            return;
        }
        *disposed = true;
        drop(disposed);
        (self.cleanup)();
    }
}

impl<K: Eq + Hash + Send + Sync + 'static> Drop for SubscriptionHandle<K> {
    fn drop(&mut self) {
        self.dispose();
    }
}

type ChannelMap<K, V> = Arc<Mutex<HashMap<(SenderId, K), mpsc::Sender<V>>>>;

type TopicMap<K> = Arc<Mutex<HashMap<SenderId, Vec<K>>>>;

pub struct SubscriptionRegistry<K: Eq + Hash + Clone + Send + Sync + 'static, V: Send + 'static> {
    channels: ChannelMap<K, V>,
    sender_topics: TopicMap<K>,
    config: SubscriptionConfig,
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Send + 'static> Default
    for SubscriptionRegistry<K, V>
{
    fn default() -> Self {
        Self::new(SubscriptionConfig::default())
    }
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Send + 'static> SubscriptionRegistry<K, V> {
    pub fn new(config: SubscriptionConfig) -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
            sender_topics: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    pub fn subscribe(
        &self,
        sender_id: SenderId,
        topic: K,
    ) -> (mpsc::Receiver<V>, SubscriptionHandle<K>) {
        let mut channels = self.channels.lock().unwrap();
        let mut sender_topics = self.sender_topics.lock().unwrap();

        if let Some(old_sender) = channels.remove(&(sender_id, topic.clone())) {
            drop(old_sender);
            if let Some(topics) = sender_topics.get_mut(&sender_id) {
                topics.retain(|t| t != &topic);
            }
        }

        let (tx, rx) = mpsc::channel(self.config.capacity);
        channels.insert((sender_id, topic.clone()), tx);
        sender_topics
            .entry(sender_id)
            .or_default()
            .push(topic.clone());

        let registry_channels = Arc::clone(&self.channels);
        let registry_sender_topics = Arc::clone(&self.sender_topics);
        let cleanup_topic = topic.clone();

        let cleanup = move || {
            let mut chans = registry_channels.lock().unwrap();
            let mut stops = registry_sender_topics.lock().unwrap();
            chans.remove(&(sender_id, cleanup_topic.clone()));
            if let Some(topics) = stops.get_mut(&sender_id) {
                topics.retain(|t| t != &cleanup_topic);
                if topics.is_empty() {
                    stops.remove(&sender_id);
                }
            }
        };

        let handle = SubscriptionHandle {
            cleanup: Box::new(cleanup),
            disposed: Mutex::new(false),
            _marker: std::marker::PhantomData,
        };

        (rx, handle)
    }

    pub async fn send(&self, sender_id: SenderId, topic: &K, value: V) -> SendResult {
        let tx = {
            let channels = self.channels.lock().unwrap();
            match channels.get(&(sender_id, topic.clone())) {
                Some(tx) => tx.clone(),
                None => return SendResult::NoSuchSubscription,
            }
        };

        match self.config.overflow {
            OverflowBehavior::Block => {
                if tx.send(value).await.is_err() {
                    SendResult::Closed
                } else {
                    SendResult::Sent
                }
            }
            OverflowBehavior::DropNewest | OverflowBehavior::DropOldest => {
                // Note: tokio::sync::mpsc::Sender does not support try_recv.
                // Therefore, DropOldest cannot be implemented purely on the sender side.
                // It behaves identically to DropNewest here: if the channel is full,
                // the new message is dropped. True DropOldest requires receiver-side
                // coordination or a custom channel primitive.
                match tx.try_send(value) {
                    Ok(()) => SendResult::Sent,
                    Err(mpsc::error::TrySendError::Full(_)) => SendResult::Dropped,
                    Err(mpsc::error::TrySendError::Closed(_)) => SendResult::Closed,
                }
            }
        }
    }

    pub fn teardown_sender(&self, sender_id: SenderId) {
        let mut channels = self.channels.lock().unwrap();
        let mut sender_topics = self.sender_topics.lock().unwrap();

        if let Some(topics) = sender_topics.remove(&sender_id) {
            for topic in topics {
                channels.remove(&(sender_id, topic));
            }
        }
    }
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Send + 'static> Clone
    for SubscriptionRegistry<K, V>
{
    fn clone(&self) -> Self {
        Self {
            channels: Arc::clone(&self.channels),
            sender_topics: Arc::clone(&self.sender_topics),
            config: self.config,
        }
    }
}
