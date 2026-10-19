//! Purpose: owned Slack Socket Mode transport with immediate envelope ACK, bounded
//! redelivery suppression, ping/pong liveness and reconnect after warning, close or failure.
//! Scope: one bot run, dedup state persists across socket reconnects but not process restarts.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/slack/api.rs
//! `-- ecosystem/src/slack/mod.rs
//! External:
//! |-- futures_util::SinkExt
//! |-- futures_util::StreamExt
//! |-- mpsc::Sender
//! |-- mpsc::error::TrySendError::Closed
//! |-- mpsc::error::TrySendError::Full
//! |-- serde::Deserialize
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- tokio::net::TcpStream
//! |-- tokio::select
//! |-- tokio::sync::mpsc
//! |-- tokio::time::MissedTickBehavior::Delay
//! |-- tokio::time::interval
//! |-- tokio::time::timeout
//! |-- tokio_tungstenite::MaybeTlsStream
//! |-- tokio_tungstenite::WebSocketStream
//! |-- tokio_tungstenite::tungstenite::Message
//! |-- tokio_util::sync::CancellationToken
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/slack/bot.rs
//! |-- ecosystem/src/slack/mod.rs
//! `-- ecosystem/tests/slack_socket.rs
//! Invariants: ACK precedes handler work, duplicate ACKs never rerun handlers, saturation
//! leaves the new envelope unacknowledged and returns QueueFull rather than dropping it.
//! Contract tests: ecosystem/tests/slack_socket.rs

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::SlackError;
use super::api::SlackApi;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone, Debug)]
pub struct SocketConfig {
    pub reconnect_base: Duration,
    pub reconnect_max: Duration,
    pub connect_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
    pub dedup_ttl: Duration,
    pub dedup_capacity: usize,
    pub max_frame_bytes: usize,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            reconnect_base: Duration::from_millis(500),
            reconnect_max: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(5),
            heartbeat_timeout: Duration::from_secs(30),
            dedup_ttl: Duration::from_secs(300),
            dedup_capacity: 4096,
            max_frame_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Envelope {
    pub envelope_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: Value,
    #[serde(default)]
    pub retry_attempt: Option<u32>,
    #[serde(default)]
    pub retry_reason: Option<String>,
}

impl Envelope {
    fn dedup_key(&self) -> String {
        // Event IDs survive redelivery even if Slack assigns a different envelope ID.
        match self
            .payload
            .get("event_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(id) if self.kind == "events_api" => format!("event:{id}"),
            _ => format!("envelope:{}", self.envelope_id),
        }
    }
}

struct Window {
    entries: HashMap<String, Instant>,
    order: VecDeque<(String, Instant)>,
}

impl Window {
    fn prune(&mut self, config: &SocketConfig) {
        while self
            .order
            .front()
            .is_some_and(|(_, time)| time.elapsed() >= config.dedup_ttl)
            || self.order.len() > config.dedup_capacity
        {
            if let Some((key, _)) = self.order.pop_front() {
                self.entries.remove(&key);
            }
        }
    }

    fn insert(&mut self, key: String, config: &SocketConfig) {
        let time = Instant::now();
        self.entries.insert(key.clone(), time);
        self.order.push_back((key, time));
        self.prune(config);
    }
}

pub async fn run(
    api: SlackApi,
    config: SocketConfig,
    tx: mpsc::Sender<Envelope>,
    cancel: CancellationToken,
) -> Result<(), SlackError> {
    if config.heartbeat_interval.is_zero()
        || config.dedup_capacity == 0
        || config.max_frame_bytes == 0
    {
        return Err(SlackError::Protocol("invalid Socket Mode limits"));
    }
    let mut window = Window {
        entries: HashMap::new(),
        order: VecDeque::new(),
    };
    let mut failures = 0u32;
    loop {
        let connected = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            result = tokio::time::timeout(config.connect_timeout, async {
                let url = api.connection_url().await?;
                let transport = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                    .max_message_size(Some(config.max_frame_bytes))
                    .max_frame_size(Some(config.max_frame_bytes));
                tokio_tungstenite::connect_async_with_config(url, Some(transport), false).await
                    .map(|(socket, _)| socket)
                    .map_err(|_| SlackError::Transport("Slack websocket connection failed".into()))
            }) => result.unwrap_or(Err(SlackError::Timeout)),
        };
        match connected {
            Ok(mut socket) => {
                failures = 0;
                let result = receive(&mut socket, &config, &tx, &cancel, &mut window).await;
                let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
                if cancel.is_cancelled() {
                    return Ok(());
                }
                if matches!(result, Err(SlackError::QueueFull | SlackError::Cancelled)) {
                    return result;
                }
            }
            Err(error) if !error.retryable() => return Err(error),
            Err(_) => {
                tracing::warn!("Slack connection attempt failed");
            }
        }
        let delay = config
            .reconnect_base
            .saturating_mul(2u32.pow(failures.min(16)))
            .min(config.reconnect_max);
        failures = failures.saturating_add(1);
        tokio::select! { _ = cancel.cancelled() => return Ok(()), _ = tokio::time::sleep(delay) => {} }
    }
}

async fn receive(
    socket: &mut Socket,
    config: &SocketConfig,
    tx: &mpsc::Sender<Envelope>,
    cancel: &CancellationToken,
    window: &mut Window,
) -> Result<(), SlackError> {
    let mut heartbeat = tokio::time::interval(config.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_seen = Instant::now();
    let mut ready = false;
    loop {
        let message = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = heartbeat.tick() => {
                if last_seen.elapsed() >= config.heartbeat_timeout { return Ok(()); }
                send(socket, Message::Ping(Vec::new().into()), config, cancel).await?;
                continue;
            }
            frame = socket.next() => frame,
        };
        let Some(Ok(message)) = message else {
            return Ok(());
        };
        match message {
            Message::Close(_) => return Ok(()),
            Message::Ping(payload) => {
                last_seen = Instant::now();
                send(socket, Message::Pong(payload), config, cancel).await?;
            }
            Message::Pong(_) => {
                last_seen = Instant::now();
            }
            Message::Text(text) => {
                if text.len() > config.max_frame_bytes {
                    return Err(SlackError::Protocol("Slack frame exceeds configured limit"));
                }
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    tracing::warn!("Malformed Slack websocket JSON");
                    continue;
                };
                match value.get("type").and_then(Value::as_str) {
                    Some("hello") => {
                        ready = true;
                        last_seen = Instant::now();
                        continue;
                    }
                    Some("disconnect") => return Ok(()),
                    _ => {}
                }
                if !ready {
                    continue;
                }
                let Ok(envelope) = serde_json::from_value::<Envelope>(value) else {
                    continue;
                };
                if envelope.envelope_id.is_empty() {
                    continue;
                }
                window.prune(config);
                let key = envelope.dedup_key();
                let duplicate = window.entries.contains_key(&key);
                // Capacity is reserved before ACK so overload never silently loses work.
                let permit = if duplicate {
                    None
                } else {
                    Some(tx.try_reserve().map_err(|error| match error {
                        mpsc::error::TrySendError::Full(_) => SlackError::QueueFull,
                        mpsc::error::TrySendError::Closed(_) => SlackError::Cancelled,
                    })?)
                };
                let ack = json!({ "envelope_id": envelope.envelope_id, "payload": {} });
                send(
                    socket,
                    Message::Text(ack.to_string().into()),
                    config,
                    cancel,
                )
                .await?;
                if let Some(permit) = permit {
                    window.insert(key, config);
                    permit.send(envelope);
                }
            }
            _ => {}
        }
    }
}

async fn send(
    socket: &mut Socket,
    message: Message,
    config: &SocketConfig,
    cancel: &CancellationToken,
) -> Result<(), SlackError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(SlackError::Cancelled),
        result = tokio::time::timeout(config.connect_timeout, socket.send(message)) => {
            result.map_err(|_| SlackError::Timeout)?
                .map_err(|_| SlackError::Transport("Slack websocket send failed".into()))
        }
    }
}
