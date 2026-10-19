//! Purpose: delete empty scoped renderer stores and stale or excess draft stores at startup, and
//! delete a scoped store as soon as it becomes empty.
//! Scope: files directly inside the user data directory named `opencode.draft.*.dat` or
//! `opencode.workspace.*.dat`. Global and window stores are never touched.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Value
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/tests/store_cleanup.rs
//! Invariants: only files up to 128 bytes can count as empty, drafts older than 30 days or beyond
//! the 100 most recent non-empty drafts are deleted, workspace stores are deleted only when empty,
//! unreadable entries are skipped rather than failing the scan.
//! Contract tests: desktop-host/tests/store_cleanup.rs

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

pub const EMPTY_STORE_MAX_BYTES: u64 = 128;
pub const DRAFT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const DRAFT_KEEP_RECENT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Draft,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupResult {
    pub scanned: usize,
    pub deleted: Vec<String>,
}

struct Candidate {
    name: String,
    kind: StoreKind,
    modified: SystemTime,
    empty: bool,
}

/// `cleanupStoreFiles` (`store-cleanup.ts:17-61`). A missing directory scans nothing.
pub fn cleanup_store_files(user_data: &Path, now: SystemTime) -> CleanupResult {
    let candidates: Vec<Candidate> = std::fs::read_dir(user_data)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            let kind = store_kind(&name)?;
            let meta = std::fs::metadata(entry.path())
                .ok()
                .filter(|m| m.is_file())?;
            Some(Candidate {
                kind,
                modified: meta.modified().ok()?,
                empty: is_empty_store(&entry.path(), meta.len()),
                name,
            })
        })
        .collect();

    let mut stale = vec![false; candidates.len()];
    for (index, candidate) in candidates.iter().enumerate() {
        let age = now.duration_since(candidate.modified).unwrap_or_default();
        if candidate.empty || (candidate.kind == StoreKind::Draft && age > DRAFT_RETENTION) {
            stale[index] = true;
        }
    }
    let mut drafts: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].kind == StoreKind::Draft && !candidates[i].empty)
        .collect();
    drafts.sort_by(|&a, &b| candidates[b].modified.cmp(&candidates[a].modified));
    drafts
        .iter()
        .skip(DRAFT_KEEP_RECENT)
        .for_each(|&i| stale[i] = true);

    let deleted = candidates
        .iter()
        .zip(stale)
        .filter(|(_, stale)| *stale)
        .map(|(candidate, _)| {
            remove_force(&user_data.join(&candidate.name));
            candidate.name.clone()
        })
        .collect();
    CleanupResult {
        scanned: candidates.len(),
        deleted,
    }
}

/// `deleteStoreFileIfEmpty` (`store-cleanup.ts:63-73`).
pub fn delete_store_file_if_empty(user_data: &Path, name: &str) -> bool {
    if store_kind(name).is_none() {
        return false;
    }
    let file = user_data.join(name);
    let Some(meta) = std::fs::metadata(&file).ok().filter(|m| m.is_file()) else {
        return false;
    };
    if !is_empty_store(&file, meta.len()) {
        return false;
    }
    remove_force(&file);
    true
}

/// `/^opencode\.draft\..+\.dat$/` and `/^opencode\.workspace\..+\.dat$/`.
pub fn store_kind(name: &str) -> Option<StoreKind> {
    let scoped = |prefix: &str| {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".dat"))
            .is_some_and(|middle| {
                !middle.is_empty() && !middle.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            })
    };
    if scoped("opencode.draft.") {
        return Some(StoreKind::Draft);
    }
    if scoped("opencode.workspace.") {
        return Some(StoreKind::Workspace);
    }
    None
}

fn is_empty_store(file: &Path, size: u64) -> bool {
    if size > EMPTY_STORE_MAX_BYTES {
        return false;
    }
    let Ok(raw) = std::fs::read(file) else {
        return false;
    };
    let text = String::from_utf8_lossy(&raw);
    // JS String.prototype.trim also strips the BOM.
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.is_empty() {
        return true;
    }
    matches!(serde_json::from_str::<Value>(&text), Ok(Value::Object(map)) if map.is_empty())
}

fn remove_force(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, path = %path.display(), "failed to remove store file");
    }
}
