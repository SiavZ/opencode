//! Purpose: serialize each Slack thread's session creation, share and completed prompts.
//! Scope: process-local mapping, no durable restart state.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/session.rs
//! |-- ecosystem/src/slack/api.rs
//! |-- ecosystem/src/slack/mod.rs
//! `-- ecosystem/src/slack/output.rs
//! External:
//! |-- output::CREATE_ERROR
//! |-- output::PROMPT_ERROR
//! |-- output::reply
//! |-- output::tool_update
//! |-- serde_json::Value
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::Mutex::new
//! |-- tokio::time::timeout
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/slack/bot.rs
//! |-- ecosystem/src/slack/mod.rs
//! `-- ecosystem/tests/slack_bot.rs
//! Invariants: one session per channel/thread, no shared-map lock spans await, per-thread
//! gates are held only within a bounded operation timeout. Failed create remains retryable.
//! Contract tests: ecosystem/tests/slack_bot.rs

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::SlackError;
use super::api::SlackApi;
use super::output;
use crate::session::{PromptRequest, SessionEvent, SessionPort};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlackMessage {
    pub channel: String,
    pub thread: String,
    pub text: String,
}

impl SlackMessage {
    pub fn from_event(event: &Value, bot_user_id: Option<&str>) -> Option<Self> {
        if event.get("type")?.as_str()? != "message" {
            return None;
        }
        if event
            .get("subtype")
            .is_some_and(|s| s.as_str().is_some_and(|s| !s.is_empty()))
        {
            return None;
        }
        if bot_user_id.is_some_and(|id| event.get("user").and_then(Value::as_str) == Some(id)) {
            return None;
        }
        let text = event.get("text")?.as_str()?.to_string();
        if text.is_empty() {
            return None;
        }
        Some(Self {
            channel: event.get("channel")?.as_str()?.to_string(),
            thread: event
                .get("thread_ts")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| event.get("ts").and_then(Value::as_str))?
                .to_string(),
            text,
        })
    }
}

struct ThreadState {
    gate: tokio::sync::Mutex<()>,
    session_id: Mutex<Option<String>>,
    channel: String,
    thread: String,
}

pub struct ThreadRouter {
    sessions: Arc<dyn SessionPort>,
    api: SlackApi,
    threads: Mutex<HashMap<(String, String), Arc<ThreadState>>>,
    operation_timeout: Duration,
    max_threads: usize,
}

impl ThreadRouter {
    pub fn new(sessions: Arc<dyn SessionPort>, api: SlackApi) -> Self {
        Self {
            sessions,
            api,
            threads: Mutex::new(HashMap::new()),
            operation_timeout: Duration::from_secs(600),
            max_threads: 4096,
        }
    }

    pub fn with_limits(mut self, max_threads: usize, operation_timeout: Duration) -> Self {
        self.max_threads = max_threads;
        self.operation_timeout = operation_timeout;
        self
    }

    pub fn mapped_sessions(&self) -> usize {
        self.threads
            .lock()
            .expect("thread map poisoned")
            .values()
            .filter(|s| s.session_id.lock().expect("session id poisoned").is_some())
            .count()
    }

    pub async fn handle(&self, message: SlackMessage) -> Result<(), SlackError> {
        let state = {
            let mut threads = self.threads.lock().expect("thread map poisoned");
            let key = (message.channel.clone(), message.thread.clone());
            if !threads.contains_key(&key) && threads.len() >= self.max_threads {
                return Err(SlackError::ThreadLimit);
            }
            threads
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(ThreadState {
                        gate: tokio::sync::Mutex::new(()),
                        session_id: Mutex::new(None),
                        channel: message.channel.clone(),
                        thread: message.thread.clone(),
                    })
                })
                .clone()
        };
        tokio::time::timeout(self.operation_timeout, async {
            let _gate = state.gate.lock().await;
            self.process(&state, &message).await
        })
        .await
        .map_err(|_| SlackError::Timeout)?
    }

    async fn process(&self, state: &ThreadState, message: &SlackMessage) -> Result<(), SlackError> {
        let id = state
            .session_id
            .lock()
            .expect("session id poisoned")
            .clone();
        let id = match id {
            Some(id) => id,
            None => {
                let created = self
                    .sessions
                    .create(Some(&format!("Slack thread {}", state.thread)))
                    .await;
                let created = match created {
                    Ok(info) => info,
                    Err(_) => {
                        return self
                            .api
                            .post_message(&state.channel, Some(&state.thread), output::CREATE_ERROR)
                            .await;
                    }
                };
                *state.session_id.lock().expect("session id poisoned") = Some(created.id.clone());
                if let Ok(shared) = self.sessions.share(&created.id).await {
                    self.api
                        .post_optional_message(
                            &state.channel,
                            Some(&state.thread),
                            shared.share.as_ref().map(|share| share.url.as_str()),
                        )
                        .await?;
                }
                created.id
            }
        };
        let response = self
            .sessions
            .prompt(&id, &PromptRequest::text(&message.text))
            .await;
        let text = match response {
            Ok(response) => output::reply(&response),
            Err(_) => output::PROMPT_ERROR.to_string(),
        };
        self.api
            .post_message(&state.channel, Some(&state.thread), &text)
            .await
    }

    pub async fn tool_event(&self, event: &SessionEvent) -> Result<(), SlackError> {
        let Some((id, text)) = output::tool_update(event) else {
            return Ok(());
        };
        let destination = {
            let threads = self.threads.lock().expect("thread map poisoned");
            threads
                .values()
                .find(|s| s.session_id.lock().expect("session id poisoned").as_deref() == Some(id))
                .map(|s| (s.channel.clone(), s.thread.clone()))
        };
        if let Some((channel, thread)) = destination {
            // Baseline ignores tool-post failures. Record only a failure marker, not content.
            if self
                .api
                .post_message(&channel, Some(&thread), &text)
                .await
                .is_err()
            {
                tracing::warn!("Slack tool update failed");
            }
        }
        Ok(())
    }
}
