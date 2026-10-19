//! Purpose: own the native plugin interpreter and bidirectional NDJSON RPC.
//! Scope: one Location plugin bridge process.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/plugins/mod.rs
//! Invariants: callbacks never block response dispatch; shutdown kills and joins all owned tasks.
//! Contract tests: integration-adapters/tests/plugin_host.rs

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use opencode_integration_api::IntegrationError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait Callbacks: Send + Sync + 'static {
    async fn request(&self, method: &str, params: Value) -> Result<Value, IntegrationError>;
    async fn notification(&self, method: &str, params: Value);
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, IntegrationError>>>>>;

pub struct Process {
    writer: Arc<Mutex<Option<ChildStdin>>>,
    child: Mutex<Child>,
    pending: Pending,
    next: AtomicU64,
    stop: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Process {
    pub fn spawn(
        interpreter: &Path,
        bridge: &Path,
        directory: &Path,
        environment: &BTreeMap<String, String>,
        callbacks: Arc<dyn Callbacks>,
    ) -> Result<Self, IntegrationError> {
        let mut child = Command::new(interpreter)
            .arg(bridge)
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| IntegrationError::Failed {
                name: "plugin interpreter".into(),
                message: error.to_string(),
            })?;
        let writer = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| IntegrationError::Closed("plugin stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| IntegrationError::Closed("plugin stderr".into()))?;
        let pending: Pending = Arc::default();
        let stop = CancellationToken::new();
        let reader = tokio::spawn(read_loop(
            BufReader::new(stdout),
            writer.clone(),
            pending.clone(),
            stop.clone(),
            callbacks,
        ));
        let drain = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(%line, "plugin stderr");
            }
        });
        Ok(Self {
            writer,
            child: Mutex::new(child),
            pending,
            next: AtomicU64::new(1),
            stop,
            tasks: Mutex::new(vec![reader, drain]),
        })
    }

    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, IntegrationError> {
        if self.stop.is_cancelled() {
            return Err(IntegrationError::Closed("plugin host".into()));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let result = async {
            send(
                &self.writer,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await?;
            tokio::time::timeout(timeout, rx)
                .await
                .map_err(|_| IntegrationError::Timeout {
                    ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
                })?
                .map_err(|_| IntegrationError::Closed("plugin host".into()))?
        }
        .await;
        self.pending.lock().await.remove(&id);
        result
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), IntegrationError> {
        send(
            &self.writer,
            json!({"jsonrpc":"2.0","method":method,"params":params}),
        )
        .await
    }

    pub async fn close(&self) {
        self.stop.cancel();
        self.writer.lock().await.take();
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        drop(child);
        for task in self.tasks.lock().await.drain(..) {
            let _ = task.await;
        }
    }
}

async fn send(writer: &Mutex<Option<ChildStdin>>, value: Value) -> Result<(), IntegrationError> {
    let mut bytes = serde_json::to_vec(&value)
        .map_err(|error| IntegrationError::Validation(error.to_string()))?;
    bytes.push(b'\n');
    let mut writer = writer.lock().await;
    let writer = writer
        .as_mut()
        .ok_or_else(|| IntegrationError::Closed("plugin stdin".into()))?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|error| IntegrationError::Closed(error.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|error| IntegrationError::Closed(error.to_string()))
}

async fn read_loop(
    reader: BufReader<tokio::process::ChildStdout>,
    writer: Arc<Mutex<Option<ChildStdin>>>,
    pending: Pending,
    stop: CancellationToken,
    callbacks: Arc<dyn Callbacks>,
) {
    let mut lines = reader.lines();
    let mut tasks = JoinSet::new();
    loop {
        let line = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            line = lines.next_line() => line,
        };
        let Ok(Some(line)) = line else { break };
        // Plugin stdout is not trusted protocol traffic without the bridge's record separator.
        let Some(line) = line.strip_prefix('\u{1e}') else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(method) = value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            if let Some(id) = value["id"].as_u64()
                && let Some(tx) = pending.lock().await.remove(&id)
            {
                let result = if let Some(error) = value.get("error") {
                    let message = error["message"]
                        .as_str()
                        .unwrap_or("plugin error")
                        .to_owned();
                    Err(match error["code"].as_i64() {
                        Some(1002) => IntegrationError::Validation(message),
                        Some(1005) => IntegrationError::Cancelled,
                        _ => IntegrationError::Rpc {
                            code: error["code"].as_i64().unwrap_or(-32603),
                            message,
                            data: error.get("data").cloned(),
                        },
                    })
                } else {
                    Ok(value["result"].clone())
                };
                let _ = tx.send(result);
            }
            continue;
        };
        let callbacks = callbacks.clone();
        let writer = writer.clone();
        tasks.spawn(async move {
            if let Some(id) = value.get("id") {
                let response = match callbacks.request(&method, value["params"].clone()).await {
                    Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                    Err(error) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":error.to_string()}}),
                };
                let _ = send(&writer, response).await;
                return;
            }
            callbacks.notification(&method, value["params"].clone()).await;
        });
    }
    stop.cancel();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    for (_, tx) in pending.lock().await.drain() {
        let _ = tx.send(Err(IntegrationError::Closed("plugin host".into())));
    }
}
