//! Purpose: detect whether a user data directory already holds state from an earlier OpenCode
//! launch, and persist the old-layout eligibility flag derived from it.
//! Scope: one directory listing plus the settings store.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/src/store/keys.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/tests/store_install_state.rs
//! Invariants: files Electron/Chromium create on a fresh install (Local State, Crashpad, ...) do
//! not count, an existing boolean eligibility flag is never recomputed.
//! Contract tests: desktop-host/tests/store_install_state.rs

use std::path::Path;

use serde_json::Value;

use super::keys::{OLD_LAYOUT_ELIGIBLE_KEY, SETTINGS_STORE};
use super::{StoreError, StoreRegistry};

/// A directory entry as seen by [`has_existing_app_state`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// `hasExistingAppState` (`install-state.ts:1-8`).
pub fn has_existing_app_state<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> bool {
    entries.into_iter().any(|entry| {
        entry.name == SETTINGS_STORE
            || entry.name.ends_with(".dat")
            || is_window_state(&entry.name)
            || (entry.is_dir && entry.name == "opencode")
    })
}

/// Lists a directory, returning no entries when it does not exist (`onboarding.ts:13`).
pub fn list_entries(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    read.map(|entry| {
        let entry = entry?;
        Ok(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type()?.is_dir(),
        })
    })
    .collect()
}

/// `initializeOldLayoutEligibility` (`onboarding.ts:12-21`). Must run before anything else
/// writes into the user data directory, otherwise the fresh-install check sees host files.
pub fn initialize_old_layout_eligibility(stores: &StoreRegistry) -> Result<bool, StoreError> {
    let entries = list_entries(stores.user_data()).map_err(|source| StoreError::Io {
        name: SETTINGS_STORE.to_string(),
        source,
    })?;
    if let Some(Value::Bool(current)) = stores.get(SETTINGS_STORE, OLD_LAYOUT_ELIGIBLE_KEY)? {
        return Ok(current);
    }
    let eligible = has_existing_app_state(&entries);
    stores.set(
        SETTINGS_STORE,
        OLD_LAYOUT_ELIGIBLE_KEY,
        Value::Bool(eligible),
    )?;
    Ok(eligible)
}

/// `isOldLayoutEligible` (`onboarding.ts:23-25`).
pub fn is_old_layout_eligible(stores: &StoreRegistry) -> bool {
    matches!(
        stores.get(SETTINGS_STORE, OLD_LAYOUT_ELIGIBLE_KEY),
        Ok(Some(Value::Bool(true)))
    )
}

/// `/^window-state-.+\.json$/`
fn is_window_state(name: &str) -> bool {
    name.strip_prefix("window-state-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .is_some_and(|middle| {
            !middle.is_empty() && !middle.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        })
}
