//! Purpose: fake SessionPort producer for Slack fixtures, records calls and cancellable work.
//! Scope: test-only session producer, no model or opencode host is started.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/session.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::json
//! |-- tokio::sync::mpsc
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- ecosystem/tests/slack_bot.rs
//! Invariants: fixtures exercise the production Slack policy through the actual SessionPort.
//! Contract tests: ecosystem/tests/slack_bot.rs
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use opencode_ecosystem::session::{
    PromptRequest, PromptResponse, SessionError, SessionEvent, SessionInfo, SessionPort, ShareInfo,
};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct Sessions {
    pub titles: Mutex<Vec<String>>,
    pub prompts: Mutex<Vec<(String, PromptRequest)>>,
    pub shares: AtomicUsize,
    pub fail_create: AtomicBool,
    pub fail_share: AtomicBool,
    pub missing_share_url: AtomicBool,
    pub active: Arc<AtomicUsize>,
    pub peak: AtomicUsize,
    pub events: mpsc::Sender<SessionEvent>,
    pub subscription_cancel: Mutex<Option<CancellationToken>>,
    receiver: Mutex<Option<mpsc::Receiver<SessionEvent>>>,
}

impl Default for Sessions {
    fn default() -> Self {
        let (events, receiver) = mpsc::channel(16);
        Self {
            titles: Mutex::new(vec![]),
            prompts: Mutex::new(vec![]),
            shares: AtomicUsize::new(0),
            fail_create: AtomicBool::new(false),
            fail_share: AtomicBool::new(false),
            missing_share_url: AtomicBool::new(false),
            active: Arc::new(AtomicUsize::new(0)),
            peak: AtomicUsize::new(0),
            events,
            subscription_cancel: Mutex::new(None),
            receiver: Mutex::new(Some(receiver)),
        }
    }
}

fn info(id: String) -> SessionInfo {
    SessionInfo {
        id,
        title: String::new(),
        version: String::new(),
        share: None,
    }
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl SessionPort for Sessions {
    async fn create(&self, title: Option<&str>) -> Result<SessionInfo, SessionError> {
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(SessionError::Decode("fixture create failed".into()));
        }
        let id = {
            let mut titles = self.titles.lock().unwrap();
            titles.push(title.unwrap().to_string());
            format!("s{}", titles.len())
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(info(id))
    }

    async fn share(&self, id: &str) -> Result<SessionInfo, SessionError> {
        self.shares.fetch_add(1, Ordering::SeqCst);
        if self.fail_share.load(Ordering::SeqCst) {
            return Err(SessionError::Decode("fixture share failed".into()));
        }
        if self.missing_share_url.load(Ordering::SeqCst) {
            return Ok(info(id.to_string()));
        }
        let mut session = info(id.to_string());
        session.share = Some(ShareInfo {
            url: format!("https://fixture.invalid/share/{id}"),
        });
        Ok(session)
    }

    async fn prompt(
        &self,
        id: &str,
        request: &PromptRequest,
    ) -> Result<PromptResponse, SessionError> {
        self.prompts
            .lock()
            .unwrap()
            .push((id.to_string(), request.clone()));
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _active = Active(self.active.clone());
        let text = request.parts[0]["text"].as_str().unwrap();
        if text == "blocked" {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        if text == "error" {
            return Err(SessionError::Decode("fixture prompt failed".into()));
        }
        Ok(PromptResponse {
            info: if text == "legacy" {
                json!({"content":"legacy content"})
            } else {
                json!({})
            },
            parts: if text == "empty" {
                vec![]
            } else {
                vec![
                    json!({"type":"text", "text":format!("reply {text}")}),
                    json!({"type":"text","text":"second"}),
                    json!({"type":"tool"}),
                ]
            },
        })
    }

    async fn subscribe(
        &self,
        cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<SessionEvent>, SessionError> {
        *self.subscription_cancel.lock().unwrap() = Some(cancel);
        self.receiver
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| SessionError::Decode("fixture already subscribed".into()))
    }
}
