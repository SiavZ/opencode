//! Purpose: in-process host for the public share Durable Object (`SyncServer`,
//! packages/function/src/api.ts:16-112): one serialized actor per short name with ordered key/value
//! storage, an R2-compatible bucket, and WebSocket subscriber fan-out.
//! Scope: native host used by bins/hosted and tests. A Cloudflare Worker (workers-rs) host is a
//! recorded blocker in contracts/phase-13/status.md.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! `-- hosted-domain/src/sharing/public.rs
//! External:
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- serde_json::Value
//! |-- serde_json::to_string
//! |-- str::to_string
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::mpsc
//! `-- uuid::Uuid::new_v4
//! Used by:
//! rust/bins/
//! |-- hosted/src/sharing/mod.rs
//! |-- hosted/src/sharing/public.rs
//! `-- hosted/src/sharing/tests.rs
//! rust/crates/
//! `-- hosted-adapters/src/sharing/mod.rs
//! Invariants: each object processes one call at a time (DO input gate), publish order is
//! bucket put -> storage put -> broadcast, subscribe replays storage `session/*` entries before
//! live frames, subscriber queues are bounded (256) and a full or closed queue drops that
//! subscriber instead of blocking publishers.
//! Contract tests: hosted-adapters/tests/sharing_public.rs

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use opencode_hosted_domain::sharing::public::{
    Entry, clear_plan, publish_allowed, publish_object_key, replay,
};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use crate::sharing::objects::{ListOptions, ObjectStore, StoreError};

pub const SUBSCRIBER_QUEUE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicError {
    /// `throw new Error("Invalid secret")` (api.ts:87-89), surfaces as HTTP 500.
    #[error("Invalid secret")]
    InvalidSecret,
    /// `throw new Error("Invalid admin secret")` (api.ts:145).
    #[error("Invalid admin secret")]
    InvalidAdminSecret,
    #[error("{0}")]
    Store(String),
}

impl From<StoreError> for PublicError {
    fn from(error: StoreError) -> Self {
        PublicError::Store(error.to_string())
    }
}

/// Outcome of `publish`: the baseline returns a 400 Response object from the RPC on a bad key,
/// which `/share_sync` ignores (it always answers `{}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    Published { subscribers: usize },
    InvalidKey,
}

#[derive(Default)]
struct ObjectState {
    storage: BTreeMap<String, Value>,
    subscribers: Vec<mpsc::Sender<String>>,
}

impl ObjectState {
    fn session_id(&self) -> Option<String> {
        self.storage
            .get("sessionID")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn secret(&self) -> Option<String> {
        self.storage
            .get("secret")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn entries(&self) -> Vec<(String, Value)> {
        self.storage
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Namespace of `SyncServer` objects keyed by `idFromName(name)`.
#[derive(Clone)]
pub struct SyncNamespace {
    bucket: Arc<dyn ObjectStore>,
    objects: Arc<Mutex<HashMap<String, Arc<Mutex<ObjectState>>>>>,
}

impl SyncNamespace {
    pub fn new(bucket: Arc<dyn ObjectStore>) -> Self {
        Self {
            bucket,
            objects: Arc::default(),
        }
    }

    async fn object(&self, name: &str) -> Arc<Mutex<ObjectState>> {
        self.objects
            .lock()
            .await
            .entry(name.to_string())
            .or_default()
            .clone()
    }

    /// `share(sessionID)` (api.ts:68-76): first caller fixes secret and session ID.
    pub async fn share(&self, name: &str, session_id: &str) -> String {
        let object = self.object(name).await;
        let mut state = object.lock().await;
        if let Some(secret) = state.secret().filter(|s| !s.is_empty()) {
            return secret;
        }
        let secret = uuid::Uuid::new_v4().to_string();
        state
            .storage
            .insert("secret".into(), Value::String(secret.clone()));
        state
            .storage
            .insert("sessionID".into(), Value::String(session_id.into()));
        secret
    }

    /// `assertSecret` (api.ts:87-89). An unshared object has secret `undefined`, which never equals a string.
    pub async fn assert_secret(&self, name: &str, secret: &str) -> Result<(), PublicError> {
        let object = self.object(name).await;
        let state = object.lock().await;
        if state.secret().as_deref() != Some(secret) {
            return Err(PublicError::InvalidSecret);
        }
        Ok(())
    }

    /// `publish(key, content)` (api.ts:46-66).
    pub async fn publish(
        &self,
        name: &str,
        key: &str,
        content: Value,
    ) -> Result<PublishOutcome, PublicError> {
        let object = self.object(name).await;
        let mut state = object.lock().await;
        if !publish_allowed(state.session_id().as_deref(), key) {
            return Ok(PublishOutcome::InvalidKey);
        }
        let body =
            serde_json::to_string(&content).map_err(|e| PublicError::Store(e.to_string()))?;
        self.bucket.write(&publish_object_key(key), body).await?;
        state.storage.insert(key.into(), content.clone());
        let frame = serde_json::to_string(&Entry {
            key: key.into(),
            content,
        })
        .map_err(|e| PublicError::Store(e.to_string()))?;
        state
            .subscribers
            .retain(|tx| tx.try_send(frame.clone()).is_ok());
        Ok(PublishOutcome::Published {
            subscribers: state.subscribers.len(),
        })
    }

    /// `getData()` (api.ts:78-84).
    pub async fn get_data(&self, name: &str) -> Vec<Entry> {
        let object = self.object(name).await;
        let state = object.lock().await;
        replay(&state.entries())
    }

    /// `fetch()` WebSocket accept (api.ts:21-38): returns a receiver primed with replay frames.
    pub async fn subscribe(&self, name: &str) -> mpsc::Receiver<String> {
        let object = self.object(name).await;
        let mut state = object.lock().await;
        let replayed = replay(&state.entries());
        let (tx, rx) = mpsc::channel(SUBSCRIBER_QUEUE.max(replayed.len() + 1));
        for entry in replayed {
            if let Ok(frame) = serde_json::to_string(&entry) {
                let _ = tx.try_send(frame);
            }
        }
        state.subscribers.push(tx);
        rx
    }

    /// `clear()` (api.ts:94-106), including the prefix mismatch with what publish wrote.
    pub async fn clear(&self, name: &str) -> Result<(), PublicError> {
        let object = self.object(name).await;
        let mut state = object.lock().await;
        let plan = clear_plan(state.session_id().as_deref());
        let listed = self
            .bucket
            .list(ListOptions {
                prefix: plan.list_prefix,
                limit: Some(plan.list_limit),
                after: None,
                before: None,
            })
            .await?;
        for key in listed {
            self.bucket.remove(&key).await?;
        }
        self.bucket.remove(&plan.delete_info_key).await?;
        // deleteAll() wipes storage; accepted WebSockets stay open in the DO runtime.
        state.storage.clear();
        Ok(())
    }
}
