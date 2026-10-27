//! Purpose: run native console statistics, trace enrichment and local operator entrypoints.
//! Scope: worker adapters and explicitly configured transports.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/workers/logs.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-adapters/src/console/workers/stat.rs
//! External:
//! |-- reqwest::Client
//! |-- serde_json::Value
//! `-- thiserror::Error
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: no implicit external destination; files are append-only and synchronized.
//! Contract tests: hosted-adapters/tests/console_workers.rs

pub mod logs;
pub mod operators;
pub mod stat;

use super::db::DbError;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error(transparent)]
    Database(#[from] DbError),
    #[error("worker database: {0}")]
    Mysql(#[from] mysql_async::Error),
    #[error("worker input: {0}")]
    Validation(String),
    #[error("unsupported operator: {0}")]
    Unsupported(String),
    #[error("worker capture: {0}")]
    Io(#[from] std::io::Error),
    #[error("worker JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("worker transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("worker task: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("worker operation: {0}")]
    Operation(String),
}

/// Explicit output destination. HTTP endpoints are configured, never inferred from credentials.
#[derive(Clone)]
pub enum Capture {
    JsonLines(PathBuf),
    Http {
        client: reqwest::Client,
        endpoint: url::Url,
        bearer: Option<String>,
    },
}

impl Capture {
    pub async fn send(&self, value: &Value) -> Result<(), WorkerError> {
        match self {
            Self::JsonLines(path) => {
                let path = path.clone();
                let mut bytes = serde_json::to_vec(value)?;
                bytes.push(b'\n');
                tokio::task::spawn_blocking(move || {
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?;
                    file.write_all(&bytes)?;
                    file.sync_data()
                })
                .await??;
            }
            Self::Http {
                client,
                endpoint,
                bearer,
            } => {
                let request = client.post(endpoint.clone()).json(value);
                let request = if let Some(token) = bearer {
                    request.bearer_auth(token)
                } else {
                    request
                };
                request.send().await?.error_for_status()?;
            }
        }
        Ok(())
    }
}
