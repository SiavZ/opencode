//! Purpose: legacy JSON key/value store, port of packages/opencode/src/storage/storage.ts:215-317
//! (`Storage.Service`): `<data>/storage/<key...>.json` files with per-file read/write locks.
//! Scope: implements `opencode_storage_api::JsonStore`. Runs the one-time file migrations (migrate.rs)
//! lazily before the first operation, like the TS `Effect.cached` state.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/json_store.rs
//! |-- storage-api/src/lib.rs
//! |-- storage-api/src/records/collation.rs
//! |-- storage-sqlite/src/legacy_files.rs
//! `-- storage-sqlite/src/legacy_files/migrate.rs
//! External:
//! |-- async_trait::async_trait
//! |-- migrate::run
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::to_owned
//! |-- tokio::fs::create_dir_all
//! |-- tokio::fs::read_to_string
//! |-- tokio::fs::remove_file
//! |-- tokio::fs::write
//! |-- tokio::sync::OnceCell
//! |-- tokio::sync::RwLock
//! `-- tokio::task::spawn_blocking
//! Used by:
//! rust/crates/
//! `-- storage-sqlite/src/legacy_files.rs
//! Invariants: the file for a key is `path.join(dir, ...key) + ".json"` (lexically normalized like
//! node `path.join`). Reads take the file's read lock, write/update/remove its write lock, so updates
//! of one key are serialized and reads run concurrently. A missing file reads as
//! `NotFound(<absolute file>)` (message `Resource not found: <file>`), a missing remove succeeds, a
//! missing or unreadable list prefix lists as empty. List entries are `prefix + relative path minus its
//! last 5 characters`, split on the separator, skipping dot paths, sorted by `join("/")` with
//! `localeCompare`. Writes are `JSON.stringify(content, null, 2)` and create parent directories.
//! Contract tests: storage-sqlite/tests/legacy_json.rs

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_storage_api::json_store::Update;
use opencode_storage_api::records::collation::locale_compare;
use opencode_storage_api::{JsonStore, Result, StorageError};
use serde_json::Value;
use tokio::sync::{OnceCell, RwLock};

use super::js_pretty;
use super::migrate;

/// Legacy JSON store rooted at `<data dir>/storage`.
pub struct LegacyJsonStore {
    data: PathBuf,
    dir: PathBuf,
    locks: Mutex<HashMap<PathBuf, Arc<RwLock<()>>>>,
    state: OnceCell<std::result::Result<(), String>>,
}

impl LegacyJsonStore {
    /// Store for the global data directory (`Global.Path.data`). Nothing is touched until first use.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data = normalize(&data_dir.into());
        Self {
            dir: data.join("storage"),
            data,
            locks: Mutex::new(HashMap::new()),
            state: OnceCell::new(),
        }
    }

    /// `<data>/storage`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    async fn ready(&self) -> Result<()> {
        let state = self
            .state
            .get_or_init(|| async {
                let data = self.data.clone();
                tokio::task::spawn_blocking(move || migrate::run(&data))
                    .await
                    .unwrap_or_else(|error| Err(format!("storage migration task failed: {error}")))
            })
            .await;
        state.clone().map_err(StorageError::Other)
    }

    async fn target(&self, key: &[String]) -> Result<(PathBuf, Arc<RwLock<()>>)> {
        self.ready().await?;
        let mut joined = self.dir.clone();
        for part in key {
            joined.push(part);
        }
        let target = PathBuf::from(format!("{}.json", normalize(&joined).display()));
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        // RcMap with idleTimeToLive 0: drop locks nobody holds.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        let lock = locks.entry(target.clone()).or_default().clone();
        Ok((target, lock))
    }
}

#[async_trait]
impl JsonStore for LegacyJsonStore {
    async fn read(&self, key: &[String]) -> Result<Value> {
        let (target, lock) = self.target(key).await?;
        let _guard = lock.read().await;
        read_json(&target).await
    }

    async fn write(&self, key: &[String], content: &Value) -> Result<()> {
        let (target, lock) = self.target(key).await?;
        let _guard = lock.write().await;
        write_with_dirs(&target, &js_pretty(content)).await
    }

    async fn update(&self, key: &[String], update: Update) -> Result<Value> {
        let (target, lock) = self.target(key).await?;
        let _guard = lock.write().await;
        let mut content = read_json(&target).await?;
        update(&mut content);
        write_with_dirs(&target, &js_pretty(&content)).await?;
        Ok(content)
    }

    async fn remove(&self, key: &[String]) -> Result<()> {
        let (target, lock) = self.target(key).await?;
        let _guard = lock.write().await;
        match tokio::fs::remove_file(&target).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&target, &error)),
        }
    }

    async fn list(&self, prefix: &[String]) -> Result<Vec<Vec<String>>> {
        self.ready().await?;
        let mut cwd = self.dir.clone();
        for part in prefix {
            cwd.push(part);
        }
        let cwd = normalize(&cwd);
        let files = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            files_under(&cwd, &mut Vec::new(), &mut out);
            out
        })
        .await
        .unwrap_or_default();
        let mut keys: Vec<Vec<String>> = files
            .into_iter()
            .map(|parts| {
                let joined = parts.join(std::path::MAIN_SEPARATOR_STR);
                let trimmed: String = {
                    let count = joined.chars().count();
                    joined.chars().take(count.saturating_sub(5)).collect()
                };
                prefix
                    .iter()
                    .cloned()
                    .chain(trimmed.split(std::path::MAIN_SEPARATOR).map(str::to_owned))
                    .collect()
            })
            .collect();
        keys.sort_by(|a, b| locale_compare(&a.join("/"), &b.join("/")));
        Ok(keys)
    }
}

/// glob `**/*` with `nodir`, no dot entries, no symlinked directory traversal. Errors list nothing.
fn files_under(dir: &Path, rel: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        rel.push(name);
        if kind.is_dir() {
            files_under(&entry.path(), rel, out);
        } else if kind.is_file() || (kind.is_symlink() && entry.path().is_file()) {
            out.push(rel.clone());
        }
        rel.pop();
    }
}

/// Lexical normalization like node `path.join` / `path.resolve` (no symlink resolution).
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub(crate) fn io_error(path: &Path, error: &std::io::Error) -> StorageError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            StorageError::PermissionDenied(format!("{}: {error}", path.display()))
        }
        _ => StorageError::Io(format!("{}: {error}", path.display())),
    }
}

/// `readJson` with missing files mapped to `NotFound(<file>)`.
async fn read_json(target: &Path) -> Result<Value> {
    let text = match tokio::fs::read_to_string(target).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StorageError::NotFound(target.display().to_string()));
        }
        Err(error) => return Err(io_error(target, &error)),
    };
    serde_json::from_str(&text).map_err(|error| StorageError::Decode {
        column: target.display().to_string(),
        message: error.to_string(),
    })
}

/// `FSUtil.writeWithDirs`: write, creating parent directories when they are missing.
pub(crate) async fn write_with_dirs(target: &Path, content: &str) -> Result<()> {
    match tokio::fs::write(target, content).await {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(io_error(target, &error));
        }
        Err(_) => {}
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| io_error(parent, &error))?;
    }
    tokio::fs::write(target, content)
        .await
        .map_err(|error| io_error(target, &error))
}
