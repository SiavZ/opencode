//! Purpose: import user data from earlier desktop generations: the Tauri-era `.dat` stores
//! (`migrate.ts`) and, when the native host does not reuse the Electron user data directory,
//! the Electron stores, window state files and `drafts.sqlite`.
//! Scope: one-shot startup migration between explicit source and target directories. Callers
//! resolve platform directories with [`tauri_dir`] / [`electron_user_data_dir`], tests pass
//! temp dirs.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/drafts.rs
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/src/store/keys.rs
//! External:
//! |-- dirs::home_dir
//! |-- rusqlite::Connection
//! |-- rusqlite::Error
//! |-- rusqlite::OpenFlags
//! |-- rusqlite::params
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- tracing::info
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lib.rs
//! `-- desktop-host/tests/migrate.rs
//! Invariants: existing target keys and draft documents are never overwritten, both migrations
//! record a flag in the target settings store and are no-ops once it is set, an unparsable
//! source file is skipped (logged) without aborting the rest, sources are only read.
//! Contract tests: desktop-host/tests/migrate.rs

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::drafts::DRAFTS_FILE;
use crate::store::keys::{SETTINGS_STORE, TAURI_MIGRATED_KEY};
use crate::store::{StoreError, StoreRegistry, js_order};

/// Settings flag set once the Electron import ran. New key, not present in the baseline.
pub const ELECTRON_MIGRATED_KEY: &str = "electronMigrated";

/// Release channel, selects the app identifier (`constants.ts`, `index.ts:58-62`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Dev,
    Beta,
    Prod,
}

impl Channel {
    /// Tauri and Electron share identifiers. Unpackaged builds always use the dev id.
    pub fn app_id(self, packaged: bool) -> &'static str {
        match (packaged, self) {
            (false, _) | (true, Channel::Dev) => "ai.opencode.desktop.dev",
            (true, Channel::Beta) => "ai.opencode.desktop.beta",
            (true, Channel::Prod) => "ai.opencode.desktop",
        }
    }
}

/// Environment inputs for directory resolution, injected so tests never see real paths.
#[derive(Debug, Clone, Default)]
pub struct DirEnv {
    pub home: PathBuf,
    /// `%APPDATA%` on Windows.
    pub appdata: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
}

impl DirEnv {
    /// Reads the current process environment. Production only.
    pub fn from_process() -> Option<Self> {
        let var = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Some(Self {
            home: dirs::home_dir()?,
            appdata: var("APPDATA"),
            xdg_data_home: var("XDG_DATA_HOME"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
        })
    }
}

/// Target OS for directory resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Os::Mac,
            "windows" => Os::Windows,
            _ => Os::Linux,
        }
    }
}

/// `tauriDir` (`migrate.ts:13-22`).
pub fn tauri_dir(os: Os, env: &DirEnv, app_id: &str) -> PathBuf {
    match os {
        Os::Mac => env
            .home
            .join("Library")
            .join("Application Support")
            .join(app_id),
        Os::Windows => env
            .appdata
            .clone()
            .unwrap_or_else(|| env.home.join("AppData").join("Roaming"))
            .join(app_id),
        Os::Linux => env
            .xdg_data_home
            .clone()
            .unwrap_or_else(|| env.home.join(".local").join("share"))
            .join(app_id),
    }
}

/// Electron `app.getPath("appData") + appId` (`index.ts:143-146`). Electron's Linux appData is
/// `$XDG_CONFIG_HOME` or `~/.config`.
pub fn electron_user_data_dir(os: Os, env: &DirEnv, app_id: &str) -> PathBuf {
    match os {
        Os::Linux => env
            .xdg_config_home
            .clone()
            .unwrap_or_else(|| env.home.join(".config"))
            .join(app_id),
        _ => tauri_dir(os, env, app_id),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("migration io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Cannot convert undefined or null to object ({file})")]
    NullSource { file: String },
    #[error("not yet ported: {0}")]
    Unsupported(&'static str),
    #[error("draft migration failed: {0}")]
    Drafts(#[from] rusqlite::Error),
}

/// Per-file outcome, mirrors the baseline log line `migrated <file> → <store> {migrated, skipped}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub file: String,
    pub store: String,
    pub migrated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The flag was already set.
    AlreadyDone,
    /// The source directory does not exist. The flag is still set.
    NoSource,
    Migrated {
        files: Vec<FileReport>,
        /// Files that failed to parse and were skipped.
        failed: Vec<String>,
        /// Draft documents copied (Electron import only).
        draft_documents: usize,
    },
}

/// `migrate()` (`migrate.ts:69-91`): import every `*.dat` in `source` into `stores`.
/// `opencode.settings.dat` maps to the `opencode.settings` store, all others keep their name.
pub fn migrate_tauri(
    source: &Path,
    stores: &StoreRegistry,
) -> Result<MigrationOutcome, MigrateError> {
    if is_set(stores, TAURI_MIGRATED_KEY) {
        tracing::info!("tauri migration: already done, skipping");
        return Ok(MigrationOutcome::AlreadyDone);
    }
    tracing::info!(dir = %source.display(), "tauri migration: starting");
    if !source.exists() {
        tracing::info!("tauri migration: no tauri data directory found, nothing to migrate");
        stores.set(SETTINGS_STORE, TAURI_MIGRATED_KEY, Value::Bool(true))?;
        return Ok(MigrationOutcome::NoSource);
    }
    let mut files = Vec::new();
    let mut failed = Vec::new();
    for name in list_files(source)?
        .into_iter()
        .filter(|n| n.ends_with(".dat"))
    {
        let target = if name == "opencode.settings.dat" {
            SETTINGS_STORE.to_string()
        } else {
            name.clone()
        };
        match merge_file(&source.join(&name), &name, &target, stores)? {
            Some(report) => files.push(report),
            None => failed.push(name),
        }
    }
    tracing::info!("tauri migration: complete");
    stores.set(SETTINGS_STORE, TAURI_MIGRATED_KEY, Value::Bool(true))?;
    Ok(MigrationOutcome::Migrated {
        files,
        failed,
        draft_documents: 0,
    })
}

/// Imports an Electron user data directory into the native host's user data directory. Store
/// files (`opencode.settings`, `*.dat`) merge key by key without overwriting, `window-state-*.json`
/// files are copied when absent, and draft documents plus their referenced blobs are copied into
/// `drafts.sqlite` without overwriting. A source equal to the target is a no-op that still sets
/// the flag (the host reads Electron files in place).
pub fn migrate_electron(
    source: &Path,
    stores: &StoreRegistry,
) -> Result<MigrationOutcome, MigrateError> {
    if is_set(stores, ELECTRON_MIGRATED_KEY) {
        return Ok(MigrationOutcome::AlreadyDone);
    }
    let same = match (source.canonicalize(), stores.user_data().canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !source.exists() || same {
        stores.set(SETTINGS_STORE, ELECTRON_MIGRATED_KEY, Value::Bool(true))?;
        return Ok(MigrationOutcome::NoSource);
    }
    let mut files = Vec::new();
    let mut failed = Vec::new();
    for name in list_files(source)? {
        let path = source.join(&name);
        if name == SETTINGS_STORE || name == "opencode.updater" || name.ends_with(".dat") {
            match merge_file(&path, &name, &name, stores)? {
                Some(report) => files.push(report),
                None => failed.push(name),
            }
            continue;
        }
        if is_window_state(&name) {
            let target = stores.user_data().join(&name);
            if !target.exists() {
                std::fs::create_dir_all(stores.user_data()).map_err(io(stores.user_data()))?;
                std::fs::copy(&path, &target).map_err(io(&target))?;
            }
        }
    }
    let draft_documents = migrate_drafts(
        &source.join(DRAFTS_FILE),
        &stores.user_data().join(DRAFTS_FILE),
    )?;
    stores.set(SETTINGS_STORE, ELECTRON_MIGRATED_KEY, Value::Bool(true))?;
    Ok(MigrationOutcome::Migrated {
        files,
        failed,
        draft_documents,
    })
}

/// `migrateFile` (`migrate.ts:39-67`). Returns `None` when the source does not parse. A JSON
/// value that is not an object contributes its `Object.entries` (none for scalars).
fn merge_file(
    path: &Path,
    file: &str,
    target: &str,
    stores: &StoreRegistry,
) -> Result<Option<FileReport>, MigrateError> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!(file, %error, "migration: failed to read");
            return Ok(None);
        }
    };
    let value = match serde_json::from_str::<Value>(&String::from_utf8_lossy(&raw)) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(file, %error, "migration: failed to parse");
            return Ok(None);
        }
    };
    // Object.entries accepts arrays, strings, numbers and booleans, but throws on null.
    let data = match value {
        Value::Object(map) => js_order(map),
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(i, value)| (i.to_string(), value))
            .collect(),
        Value::String(value) => {
            if value.chars().any(|c| c.len_utf16() != 1) {
                return Err(MigrateError::Unsupported(
                    "Tauri string migration containing surrogate pairs",
                ));
            }
            value
                .chars()
                .enumerate()
                .map(|(i, value)| (i.to_string(), Value::String(value.to_string())))
                .collect()
        }
        Value::Null => {
            return Err(MigrateError::NullSource {
                file: file.to_string(),
            });
        }
        _ => serde_json::Map::new(),
    };
    let mut report = FileReport {
        file: file.to_string(),
        store: target.to_string(),
        migrated: Vec::new(),
        skipped: Vec::new(),
    };
    for (key, value) in data {
        if stores.has(target, &key)? {
            report.skipped.push(key);
            continue;
        }
        stores.set(target, &key, value)?;
        report.migrated.push(key);
    }
    tracing::info!(file, store = target, migrated = ?report.migrated, skipped = ?report.skipped, "migration: migrated");
    Ok(Some(report))
}

/// Copies documents absent from the target and all source blobs. Blobs no document references
/// are collected by the next `DraftStore::open` of the target.
fn migrate_drafts(source: &Path, target: &Path) -> Result<usize, MigrateError> {
    if !source.exists() {
        return Ok(0);
    }
    // Opening through DraftStore creates the schema and keeps GC semantics on the target.
    drop(
        crate::drafts::DraftStore::open(target).map_err(|error| match error {
            crate::drafts::DraftError::Sqlite(error) => MigrateError::Drafts(error),
            other => MigrateError::Io {
                path: target.to_path_buf(),
                source: std::io::Error::other(other.to_string()),
            },
        })?,
    );
    let mut src = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Documents and blobs must come from one snapshot if the source is still open in WAL mode.
    let src = src.transaction()?;
    let mut dst = Connection::open(target)?;
    let tx = dst.transaction()?;
    let mut copied = 0;
    let mut statement = src.prepare("SELECT key, value FROM document")?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (key, value) = row?;
        copied += tx.execute(
            "INSERT INTO document (key, value) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            [&key, &value],
        )?;
    }
    let mut blobs = src.prepare("SELECT id, data FROM blob")?;
    let rows = blobs.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        let (id, data) = row?;
        tx.execute(
            "INSERT INTO blob (id, data) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            rusqlite::params![id, data],
        )?;
    }
    tx.commit()?;
    Ok(copied)
}

fn is_set(stores: &StoreRegistry, key: &str) -> bool {
    // JS truthiness of `getStore().get(key)`.
    match stores.get(SETTINGS_STORE, key) {
        Ok(Some(Value::Bool(b))) => b,
        Ok(Some(Value::Null)) | Ok(None) | Err(_) => false,
        Ok(Some(Value::Number(n))) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Ok(Some(Value::String(s))) => !s.is_empty(),
        Ok(Some(_)) => true,
    }
}

fn list_files(dir: &Path) -> Result<Vec<String>, MigrateError> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(io(dir))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_type().ok()?.is_file().then_some(())?;
            entry.file_name().into_string().ok()
        })
        .collect();
    names.sort();
    Ok(names)
}

fn is_window_state(name: &str) -> bool {
    name.strip_prefix("window-state-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .is_some_and(|middle| !middle.is_empty())
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> MigrateError + use<> {
    let path = path.to_path_buf();
    move |source| MigrateError::Io { path, source }
}
