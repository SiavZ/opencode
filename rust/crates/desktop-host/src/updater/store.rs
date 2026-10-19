//! Purpose: file-backed persistence for the updater store (`opencode.updater` in the app data
//! directory, electron-store compatible JSON object). Holds the `ready` target record and the
//! selected update `channel`.
//! Scope: synchronous small-file IO with atomic replace.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/updater/controller.rs
//! `-- desktop-host/src/updater/state.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- serde_json::to_string_pretty
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! Invariants: unknown keys are preserved, a malformed `ready` value reads as absent
//! (`updater.ts:51-55`), writes go through a temp file + rename, the file is removed when the
//! store becomes empty.
//! Contract tests: desktop-host/tests/updater_store.rs

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde_json::{Map, Value};

use super::controller::UpdaterPersistence;
use super::state::{UpdaterError, UpdaterReadyRecord};

/// electron-store name used by `updater.ts:28` (no file extension).
pub const UPDATER_STORE: &str = "opencode.updater";
/// electron-updater channel set by `updater.ts:17`.
pub const DEFAULT_CHANNEL: &str = "latest";

const READY_KEY: &str = "ready";
const CHANNEL_KEY: &str = "channel";

#[derive(Debug)]
pub struct UpdaterFileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl UpdaterFileStore {
    /// `dir` is the app user-data directory. The store file is `<dir>/opencode.updater`.
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            path: dir.as_ref().join(UPDATER_STORE),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persisted channel, `latest` when unset or invalid.
    pub fn channel(&self) -> String {
        let _guard = self.lock.lock();
        read(&self.path)
            .ok()
            .and_then(|map| {
                map.get(CHANNEL_KEY)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|channel| valid_channel(channel))
            .unwrap_or_else(|| DEFAULT_CHANNEL.to_owned())
    }

    pub fn set_channel(&self, channel: &str) -> Result<(), UpdaterError> {
        if !valid_channel(channel) {
            return Err(UpdaterError::InvalidChannel(channel.to_owned()));
        }
        let _guard = self.lock.lock();
        let mut map = read(&self.path)?;
        map.insert(CHANNEL_KEY.to_owned(), Value::String(channel.to_owned()));
        write(&self.path, &map)
    }

    fn update(&self, f: impl FnOnce(&mut Map<String, Value>)) -> Result<(), UpdaterError> {
        let _guard = self.lock.lock();
        let mut map = read(&self.path)?;
        f(&mut map);
        write(&self.path, &map)
    }
}

impl UpdaterPersistence for UpdaterFileStore {
    fn get(&self) -> Result<Option<UpdaterReadyRecord>, UpdaterError> {
        let _guard = self.lock.lock();
        let map = read(&self.path)?;
        Ok(map
            .get(READY_KEY)
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(|version| UpdaterReadyRecord {
                version: version.to_owned(),
            }))
    }

    fn set(&self, value: &UpdaterReadyRecord) -> Result<(), UpdaterError> {
        self.update(|map| {
            map.insert(
                READY_KEY.to_owned(),
                serde_json::json!({ "version": value.version }),
            );
        })
    }

    fn clear(&self) -> Result<(), UpdaterError> {
        self.update(|map| {
            map.remove(READY_KEY);
        })
    }
}

/// Channel names become a feed URL path segment, keep them to a safe alphabet.
fn valid_channel(channel: &str) -> bool {
    !channel.is_empty()
        && channel.len() <= 64
        && channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn read(path: &Path) -> Result<Map<String, Value>, UpdaterError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(UpdaterError::Persistence(error.to_string())),
    };
    // electron-store resets a corrupt file to defaults, match that instead of failing startup.
    Ok(serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default())
}

fn write(path: &Path, map: &Map<String, Value>) -> Result<(), UpdaterError> {
    let error = |e: std::io::Error| UpdaterError::Persistence(e.to_string());
    if map.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(error(e)),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(error)?;
    }
    let body =
        serde_json::to_string_pretty(map).map_err(|e| UpdaterError::Persistence(e.to_string()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(error)?;
    std::fs::rename(&tmp, path).map_err(error)
}
