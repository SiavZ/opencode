//! Purpose: named key/value stores persisted exactly like electron-store 11 / conf 15 files
//! (`<userData>/<name>`, no extension, tab-indented JSON) so existing Electron user data reads
//! and writes interchangeably, plus the `store-*` IPC semantics.
//! Scope: settings and renderer persistence files in one user data directory.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/store/cleanup.rs
//! |-- desktop-host/src/store/debug_logs.rs
//! |-- desktop-host/src/store/install_state.rs
//! `-- desktop-host/src/store/keys.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- serde::Serialize::serialize
//! |-- serde_json::Error
//! |-- serde_json::Map
//! |-- serde_json::Serializer::with_formatter
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::ser::PrettyFormatter::with_indent
//! |-- serde_json::to_string
//! |-- tracing::warn
//! `-- u32::MAX
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lib.rs
//! |-- desktop-host/src/migrate.rs
//! |-- desktop-host/src/store/install_state.rs
//! |-- desktop-host/tests/migrate.rs
//! |-- desktop-host/tests/store.rs
//! `-- desktop-host/tests/store_install_state.rs
//! Invariants: every operation re-reads the file (no stale cache across processes), writes are
//! atomic (temp file + rename) and serialized per store name, `__internal__` survives rewrites
//! and cannot be set, `__proto__`/`constructor`/`prototype` sets are ignored, a missing file is
//! an empty store, an unparsable file is an error (conf `clearInvalidConfig: false`), store names
//! that would escape the user data directory are rejected.
//! Contract tests: desktop-host/tests/store.rs

pub mod cleanup;
pub mod debug_logs;
pub mod install_state;
pub mod keys;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value};

pub use keys::SETTINGS_STORE;

/// conf's reserved key for its own bookkeeping (`conf/dist/source/index.js:36`).
pub const INTERNAL_KEY: &str = "__internal__";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error on store {name:?}: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
    /// The file exists but is not JSON. conf rethrows the SyntaxError.
    #[error("store {name:?} is not valid JSON: {source}")]
    Corrupt {
        name: String,
        #[source]
        source: serde_json::Error,
    },
    /// The file parsed but is not a JSON object.
    #[error("store {name:?} does not contain a JSON object")]
    NotObject { name: String },
    #[error(
        "Please don't use the __internal__ key, as it's used to manage this module internal operations."
    )]
    ReservedKey,
    /// Hardening beyond the baseline: Electron resolved arbitrary names against userData.
    #[error("invalid store name {0:?}")]
    InvalidName(String),
}

/// Registry of named stores under one user data directory (`store.ts:15-27`).
pub struct StoreRegistry {
    root: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl StoreRegistry {
    pub fn new(user_data: impl Into<PathBuf>) -> Self {
        Self {
            root: user_data.into(),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn user_data(&self) -> &Path {
        &self.root
    }

    /// Path of a named store. electron-store uses `fileExtension: ""`, so the name is the file.
    pub fn path(&self, name: &str) -> Result<PathBuf, StoreError> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\\', '\0'])
            || Path::new(name).is_absolute()
        {
            return Err(StoreError::InvalidName(name.to_string()));
        }
        Ok(self.root.join(name))
    }

    /// Full contents of the store, `{}` when the file does not exist.
    pub fn read(&self, name: &str) -> Result<Map<String, Value>, StoreError> {
        let path = self.path(name)?;
        let lock = self.lock_for(name);
        let _guard = lock.lock();
        read_file(name, &path)
    }

    pub fn get(&self, name: &str, key: &str) -> Result<Option<Value>, StoreError> {
        Ok(self.read(name)?.remove(key))
    }

    pub fn has(&self, name: &str, key: &str) -> Result<bool, StoreError> {
        Ok(self.read(name)?.contains_key(key))
    }

    /// conf `set(key, value)` with `accessPropertiesByDotNotation: false`.
    pub fn set(&self, name: &str, key: &str, value: Value) -> Result<(), StoreError> {
        if is_reserved(key) || contains_reserved(&value) {
            return Err(StoreError::ReservedKey);
        }
        self.update(name, |store| {
            if !matches!(key, "__proto__" | "constructor" | "prototype") {
                store.insert(key.to_string(), value);
            }
        })
    }

    pub fn delete(&self, name: &str, key: &str) -> Result<(), StoreError> {
        self.update(name, |store| {
            store.shift_remove(key);
        })
    }

    /// conf `clear()`: no defaults are configured, so the store becomes `{}` (plus preserved
    /// `__internal__`).
    pub fn clear(&self, name: &str) -> Result<(), StoreError> {
        self.update(name, Map::clear)
    }

    /// `Object.keys(store.store)` including any `__internal__` key, in JS property order.
    pub fn keys(&self, name: &str) -> Result<Vec<String>, StoreError> {
        Ok(js_order(self.read(name)?)
            .into_iter()
            .map(|(key, _)| key)
            .collect())
    }

    pub fn len(&self, name: &str) -> Result<usize, StoreError> {
        Ok(self.read(name)?.len())
    }

    /// `removeStoreFile` (`store.ts:32-35`): unconditional, missing file is fine.
    pub fn remove_file(&self, name: &str) -> Result<(), StoreError> {
        let path = self.path(name)?;
        let lock = self.lock_for(name);
        let _guard = lock.lock();
        match std::fs::remove_file(&path) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => Err(StoreError::Io {
                name: name.to_string(),
                source,
            }),
            _ => Ok(()),
        }
    }

    /// `removeStoreFileIfEmpty` (`store.ts:28-30`): only scoped draft/workspace stores.
    pub fn remove_file_if_empty(&self, name: &str) -> Result<bool, StoreError> {
        self.path(name)?;
        let lock = self.lock_for(name);
        let _guard = lock.lock();
        Ok(cleanup::delete_store_file_if_empty(&self.root, name))
    }

    // IPC handlers (`ipc.ts:114-142`).

    /// `store-get`: strings are returned raw, other values JSON-encoded, null/missing and every
    /// error (unreadable or corrupt file) map to `None`.
    pub fn ipc_get(&self, name: &str, key: &str) -> Option<String> {
        match self.get(name, key).ok()?? {
            Value::Null => None,
            Value::String(value) => Some(value),
            other => serde_json::to_string(&other).ok(),
        }
    }

    pub fn ipc_set(&self, name: &str, key: &str, value: &str) -> Result<(), StoreError> {
        self.set(name, key, Value::String(value.to_string()))
    }

    /// `store-delete`: delete, then drop the file when a scoped store became empty. The baseline
    /// fires the removal without awaiting it, so its failure is logged, not returned.
    pub fn ipc_delete(&self, name: &str, key: &str) -> Result<(), StoreError> {
        self.delete(name, key)?;
        self.remove_if_empty_logged(name);
        Ok(())
    }

    pub fn ipc_clear(&self, name: &str) -> Result<(), StoreError> {
        self.clear(name)?;
        self.remove_if_empty_logged(name);
        Ok(())
    }

    fn remove_if_empty_logged(&self, name: &str) {
        if let Err(error) = self.remove_file_if_empty(name) {
            tracing::warn!(%error, name, "failed to remove empty store file");
        }
    }

    fn update(
        &self,
        name: &str,
        edit: impl FnOnce(&mut Map<String, Value>),
    ) -> Result<(), StoreError> {
        let path = self.path(name)?;
        let lock = self.lock_for(name);
        let _guard = lock.lock();
        let current = read_file(name, &path)?;
        let internal = current.get(INTERNAL_KEY).cloned();
        let mut next = current;
        edit(&mut next);
        // conf `set store` preserves `__internal__` when the new value lacks it.
        if let Some(internal) = internal
            && !next.contains_key(INTERNAL_KEY)
        {
            next.insert(INTERNAL_KEY.to_string(), internal);
        }
        write_file(name, &path, &Value::Object(next))
    }

    fn lock_for(&self, name: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .entry(name.to_string())
            .or_default()
            .clone()
    }
}

/// Reads a conf store file. Missing file creates the directory and returns `{}`.
pub(crate) fn read_file(name: &str, path: &Path) -> Result<Map<String, Value>, StoreError> {
    let io = |source| StoreError::Io {
        name: name.to_string(),
        source,
    };
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            return Ok(Map::new());
        }
        Err(error) => return Err(io(error)),
    };
    let text = String::from_utf8_lossy(&raw);
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(js_order(map)),
        Ok(_) => Err(StoreError::NotObject {
            name: name.to_string(),
        }),
        Err(source) => Err(StoreError::Corrupt {
            name: name.to_string(),
            source,
        }),
    }
}

/// `JSON.stringify(value, undefined, '\t')` written atomically (temp file + rename).
pub(crate) fn write_file(name: &str, path: &Path, value: &Value) -> Result<(), StoreError> {
    let io = |source| StoreError::Io {
        name: name.to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let data = to_js_json(value);
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|f| f.to_str()).unwrap_or("store"),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// Serializes like `JSON.stringify(value, undefined, '\t')`, including JS property ordering
/// (array-index keys first, ascending) at every depth.
pub fn to_js_json(value: &Value) -> String {
    let ordered = order_deep(value.clone());
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&ordered, &mut serializer)
        .expect("serializing a Value cannot fail");
    String::from_utf8(out).expect("serde_json emits UTF-8")
}

fn order_deep(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            js_order(map)
                .into_iter()
                .map(|(key, value)| (key, order_deep(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(order_deep).collect()),
        // JS has one number type: `1.0` round-trips as `1`.
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_i64() && !n.is_u64() && f.fract() == 0.0 && f.abs() < 9.0e15 => {
                Value::from(f as i64)
            }
            _ => Value::Number(n),
        },
        other => other,
    }
}

/// JS own-property order: canonical array indices (0..2^32-2) ascending, then insertion order.
pub fn js_order(map: Map<String, Value>) -> Map<String, Value> {
    let index = |key: &str| -> Option<u32> {
        let n: u32 = key.parse().ok()?;
        (n != u32::MAX && n.to_string() == key).then_some(n)
    };
    if !map.keys().any(|key| index(key).is_some()) {
        return map;
    }
    let (mut indexed, rest): (Vec<_>, Vec<_>) =
        map.into_iter().partition(|(key, _)| index(key).is_some());
    indexed.sort_by_key(|(key, _)| index(key));
    indexed.into_iter().chain(rest).collect()
}

fn is_reserved(key: &str) -> bool {
    key == INTERNAL_KEY || key.starts_with("__internal__.")
}

fn contains_reserved(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| is_reserved(key) || contains_reserved(value)),
        _ => false,
    }
}
