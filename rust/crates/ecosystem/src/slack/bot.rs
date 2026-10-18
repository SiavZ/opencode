//! Purpose: compose Socket Mode ingress, per-thread SessionPort work and live tool output.
//! Scope: one awaited bot run, all locally spawned tasks are cancelled and joined on exit.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/session.rs
//! |-- ecosystem/src/slack/api.rs
//! |-- ecosystem/src/slack/mod.rs
//! |-- ecosystem/src/slack/output.rs
//! |-- ecosystem/src/slack/socket.rs
//! `-- ecosystem/src/slack/thread.rs
//! External:
//! |-- tokio::{sync::mpsc, task::JoinSet}
//! |-- tokio_util::sync::CancellationToken
//! `-- tracing::warn
//! Used by:
//! `-- none, reserved contract (owner: phase-14, hosted Slack composition)
//! Invariants: socket ACK never awaits session work, workers/queues are bounded, raw
//! message bodies and provider responses are never logged. Caller owns SessionPort host.
//! Contract tests: ecosystem/tests/slack_bot.rs, ecosystem/tests/slack_socket.rs

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::SlackError;
use super::api::SlackApi;
use super::output;
use super::socket::{self, Envelope, SocketConfig};
use super::thread::{SlackMessage, ThreadRouter};
use crate::session::SessionPort;

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub socket: SocketConfig,
    pub queue_capacity: usize,
    pub max_workers: usize,
    pub startup_timeout: Duration,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            socket: SocketConfig::default(),
            queue_capacity: 256,
            max_workers: 32,
            startup_timeout: Duration::from_secs(30),
        }
    }
}

pub struct SlackBot {
    api: SlackApi,
    sessions: Arc<dyn SessionPort>,
    pub threads: Arc<ThreadRouter>,
    config: BotConfig,
}

impl SlackBot {
    pub fn new(api: SlackApi, sessions: Arc<dyn SessionPort>, config: BotConfig) -> Self {
        let threads = Arc::new(ThreadRouter::new(sessions.clone(), api.clone()));
        Self {
            api,
            sessions,
            threads,
            config,
        }
    }

    pub async fn run(&self, cancel: CancellationToken) -> Result<(), SlackError> {
        if self.config.queue_capacity == 0 || self.config.max_workers == 0 {
            return Err(SlackError::Protocol("invalid Slack bot limits"));
        }
        let owned = cancel.child_token();
        let _shutdown = owned.clone().drop_guard();
        let startup = tokio::select! {
            _ = owned.cancelled() => return Ok(()),
            result = tokio::time::timeout(self.config.startup_timeout, async {
                let identity = self.api.bot_user_id().await?;
                let events = self.sessions.subscribe(owned.clone()).await?;
                Ok::<_, SlackError>((identity, events))
            }) => result.map_err(|_| SlackError::Timeout)?,
        };
        let (identity, mut events) = match startup {
            Ok(startup) => startup,
            Err(error) => {
                owned.cancel();
                return Err(error);
            }
        };
        let (tx, mut rx) = mpsc::channel(self.config.queue_capacity);
        let mut socket = tokio::spawn(socket::run(
            self.api.clone(),
            self.config.socket.clone(),
            tx,
            owned.clone(),
        ));
        let mut workers = JoinSet::new();
        let mut events_open = true;
        let mut ingress_open = true;
        let (result, socket_joined) = loop {
            tokio::select! {
                _ = owned.cancelled() => break (Ok(()), false),
                result = &mut socket => break (result.map_err(|_| SlackError::Task).and_then(|r| r), true),
                envelope = rx.recv(), if ingress_open && workers.len() < self.config.max_workers => {
                    let Some(envelope) = envelope else { ingress_open = false; continue; };
                    let threads = self.threads.clone();
                    let api = self.api.clone();
                    let identity = identity.clone();
                    let cancel = owned.clone();
                    workers.spawn(async move {
                        tokio::select! {
                            _ = cancel.cancelled() => Ok(()),
                            result = dispatch(&threads, &api, &identity, envelope) => result,
                        }
                    });
                }
                event = events.recv(), if events_open && workers.len() < self.config.max_workers => {
                    let Some(event) = event else { events_open = false; continue; };
                    let threads = self.threads.clone();
                    let cancel = owned.clone();
                    workers.spawn(async move {
                        tokio::select! {
                            _ = cancel.cancelled() => Ok(()),
                            result = threads.tool_event(&event) => result,
                        }
                    });
                }
                result = workers.join_next(), if !workers.is_empty() => {
                    match result {
                        Some(Err(_)) => break (Err(SlackError::Task), false),
                        Some(Ok(Err(_))) => tracing::warn!("Slack handler failed"),
                        _ => {}
                    }
                }
            }
        };
        owned.cancel();
        drop(events);
        drop(rx);
        if !socket_joined {
            let _ = socket.await;
        }
        while workers.join_next().await.is_some() {}
        result
    }
}

async fn dispatch(
    threads: &ThreadRouter,
    api: &SlackApi,
    identity: &str,
    envelope: Envelope,
) -> Result<(), SlackError> {
    match envelope.kind.as_str() {
        "events_api" => {
            if let Some(message) = envelope
                .payload
                .get("event")
                .and_then(|event| SlackMessage::from_event(event, Some(identity)))
            {
                threads.handle(message).await?;
            }
        }
        "slash_commands"
            if envelope.payload.get("command").and_then(|v| v.as_str()) == Some("/test") =>
        {
            let channel = envelope
                .payload
                .get("channel_id")
                .and_then(|v| v.as_str())
                .ok_or(SlackError::Protocol("slash command has no channel"))?;
            api.post_message(channel, None, output::TEST_RESPONSE)
                .await?;
        }
        _ => {}
    }
    Ok(())
}
