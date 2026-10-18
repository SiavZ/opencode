//! Purpose: first-launch onboarding, old-layout eligibility and default server URL settings
//! (`main/onboarding.ts`, `main/install-state.ts`, `main/server.ts:30-42`, `main/store-keys.ts`).
//! Scope: decision logic over a settings store trait. The durable store adapter is owned by
//! p11.persistence (`store.rs`); this module never opens the real user data directory itself.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Value
//! `-- tracing
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lifecycle.rs
//! `-- desktop-host/tests/lifecycle_onboarding.rs
//! Invariants: old-layout eligibility is computed once and then sticky (a stored boolean wins);
//! onboarding is pending unless the stored flag is exactly `true`; finishing twice returns `None`
//! and creates nothing; an empty default server URL deletes the key.
//! Contract tests: desktop-host/tests/lifecycle_onboarding.rs

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const SETTINGS_STORE: &str = "opencode.settings";
pub const DEFAULT_SERVER_URL_KEY: &str = "defaultServerUrl";
pub const FIRST_LAUNCH_ONBOARDING_COMPLETE_KEY: &str = "firstLaunchOnboardingComplete";
pub const OLD_LAYOUT_ELIGIBLE_KEY: &str = "oldLayoutEligible";
pub const DEFAULT_PROJECT_DIR: &str = "Default Project";

/// Minimal view of the `opencode.settings` store used by lifecycle decisions.
pub trait SettingsStore {
    fn get(&self, key: &str) -> Option<Value>;
    fn set(&mut self, key: &str, value: Value);
    fn delete(&mut self, key: &str);
}

/// A directory entry as seen by `hasExistingAppState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
}

/// `install-state.ts:1-8`: state an earlier OpenCode launch would have written.
pub fn has_existing_app_state(entries: &[DirEntryInfo]) -> bool {
    entries.iter().any(|entry| {
        let name = entry.name.as_str();
        if name == SETTINGS_STORE || name.ends_with(".dat") {
            return true;
        }
        if is_window_state_file(name) {
            return true;
        }
        entry.is_dir && name == "opencode"
    })
}

/// `/^window-state-.+\.json$/`
fn is_window_state_file(name: &str) -> bool {
    name.strip_prefix("window-state-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .is_some_and(|middle| !middle.is_empty() && !middle.contains('\n'))
}

/// Lists `user_data` like `readdirSync(..., { withFileTypes: true })`; a missing dir is empty.
pub fn read_entries(user_data: &Path) -> io::Result<Vec<DirEntryInfo>> {
    if !user_data.exists() {
        return Ok(Vec::new());
    }
    std::fs::read_dir(user_data)?
        .map(|entry| {
            let entry = entry?;
            Ok(DirEntryInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: entry.file_type()?.is_dir(),
            })
        })
        .collect()
}

/// `initializeOldLayoutEligibility` (`onboarding.ts:12-21`). Must run before anything writes
/// into `user_data` on a fresh install, otherwise every install looks like an upgrade.
pub fn initialize_old_layout_eligibility(
    store: &mut dyn SettingsStore,
    user_data: &Path,
) -> io::Result<bool> {
    let entries = read_entries(user_data)?;
    if let Some(Value::Bool(current)) = store.get(OLD_LAYOUT_ELIGIBLE_KEY) {
        return Ok(current);
    }
    let eligible = has_existing_app_state(&entries);
    store.set(OLD_LAYOUT_ELIGIBLE_KEY, Value::Bool(eligible));
    Ok(eligible)
}

pub fn is_old_layout_eligible(store: &dyn SettingsStore) -> bool {
    store.get(OLD_LAYOUT_ELIGIBLE_KEY) == Some(Value::Bool(true))
}

pub fn is_first_launch_onboarding_pending(store: &dyn SettingsStore) -> bool {
    let pending = store.get(FIRST_LAUNCH_ONBOARDING_COMPLETE_KEY) != Some(Value::Bool(true));
    tracing::info!(target: "onboarding", pending, "first launch onboarding pending checked");
    pending
}

/// `finishFirstLaunchOnboarding` (`onboarding.ts:33-45`). Returns the created default project
/// directory, or `None` when not requested or already completed. The flag is written only after
/// the directory exists, so a failed mkdir leaves onboarding pending.
pub fn finish_first_launch_onboarding(
    store: &mut dyn SettingsStore,
    documents_dir: &Path,
    create_default_project: bool,
) -> io::Result<Option<PathBuf>> {
    if !is_first_launch_onboarding_pending(store) {
        tracing::info!(target: "onboarding", "first launch onboarding already completed");
        return Ok(None);
    }
    let default_project = create_default_project.then(|| documents_dir.join(DEFAULT_PROJECT_DIR));
    if let Some(dir) = &default_project {
        std::fs::create_dir_all(dir)?;
    }
    store.set(FIRST_LAUNCH_ONBOARDING_COMPLETE_KEY, Value::Bool(true));
    tracing::info!(
        target: "onboarding",
        create_default_project,
        default_project = ?default_project,
        "first launch onboarding completed"
    );
    Ok(default_project)
}

/// `getDefaultServerUrl`: non-string stored values read as `None`.
pub fn default_server_url(store: &dyn SettingsStore) -> Option<String> {
    match store.get(DEFAULT_SERVER_URL_KEY) {
        Some(Value::String(url)) => Some(url),
        _ => None,
    }
}

/// `setDefaultServerUrl`: `None` or empty string (JS falsy) deletes the key.
pub fn set_default_server_url(store: &mut dyn SettingsStore, url: Option<&str>) {
    match url {
        Some(url) if !url.is_empty() => {
            store.set(DEFAULT_SERVER_URL_KEY, Value::String(url.into()))
        }
        _ => store.delete(DEFAULT_SERVER_URL_KEY),
    }
}
