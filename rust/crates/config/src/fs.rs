//! Purpose: narrow std-backed filesystem helpers used by config discovery: safe reads (missing or
//! permission-denied read as absent), `FSUtil.up` ancestor walks, and sorted glob scans.
//! Scope: blocking filesystem adapter.
//! Uses:
//! rust/crates/
//! `-- config/src/error.rs
//! External:
//! `-- std::fs
//! Used by:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/update.rs
//! `-- config/src/plugins.rs
//! Invariants: `up` checks targets in order per directory and stops at `stop` or the root, like
//! `packages/core/src/fs-util.ts:168-182`. platform-api has no read/glob contract yet (phase 05
//! notified), so this stays internal.
//! Contract tests: config/tests/current.rs

use crate::error::ConfigError;
use std::path::{Path, PathBuf};

/// `readFileStringSafe`: `Ok(None)` for not-found and permission-denied.
pub fn read_safe(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::IsADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) if path.is_dir() => {
            let _ = error;
            Ok(None)
        }
        Err(error) => Err(ConfigError::io("read", path, &error)),
    }
}

pub fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// `FSUtil.up({ targets, start, stop })`.
pub fn up(targets: &[&str], start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        for target in targets {
            let search = current.join(target);
            if exists(&search) {
                result.push(search);
            }
        }
        if stop.is_some_and(|stop| stop == current) {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    result
}

/// `{a,b}/*.{x,y}` (or `{a,b}/**/*.{x,y}` when `recursive`) under `cwd`: dot files included,
/// symlinks followed, files only, sorted absolute paths.
pub fn glob(cwd: &Path, dirs: &[&str], recursive: bool, extensions: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        walk(&cwd.join(dir), recursive, extensions, &mut out, 0);
    }
    out.sort();
    out
}

fn walk(dir: &Path, recursive: bool, extensions: &[&str], out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if recursive {
                walk(&path, recursive, extensions, out, depth + 1);
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        if meta.is_file() && extensions.contains(&ext) {
            out.push(path);
        }
    }
}

/// `path.relative(dir, file)` with `/` separators.
pub fn relative_slash(dir: &Path, file: &Path) -> String {
    file.strip_prefix(dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}
