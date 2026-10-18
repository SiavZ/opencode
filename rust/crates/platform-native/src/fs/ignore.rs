//! Purpose: default watcher/scan ignore rules (`packages/core/src/filesystem/ignore.ts`) and
//! OS-protected paths that must never be watched, stated or scanned
//! (`packages/core/src/filesystem/protected.ts`).
//! Scope: pure matching plus home-directory lookup.
//! Uses:
//! rust/crates/
//! `-- platform-native/src/fs/glob.rs
//! External:
//! `-- dirs::home_dir
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! |-- platform-native/src/fs/watcher.rs
//! `-- platform-native/tests/fs_util.rs
//! Invariants: whitelist wins over every ignore rule; any path component equal to an ignored folder
//! name ignores the path; file patterns use minimatch `{ dot: true }` semantics; `PATTERNS` order is
//! FILES then FOLDERS like the baseline.
//! Contract tests: platform-native/tests/fs_util.rs

use super::glob::glob_match;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const FOLDERS: &[&str] = &[
    "node_modules",
    "bower_components",
    ".pnpm-store",
    "vendor",
    ".npm",
    "dist",
    "build",
    "out",
    ".next",
    "target",
    "bin",
    "obj",
    ".git",
    ".svn",
    ".hg",
    ".vscode",
    ".idea",
    ".turbo",
    ".output",
    "desktop",
    ".sst",
    ".cache",
    ".webkit-cache",
    "__pycache__",
    ".pytest_cache",
    "mypy_cache",
    ".history",
    ".gradle",
];

pub const FILES: &[&str] = &[
    "**/*.swp",
    "**/*.swo",
    "**/*.pyc",
    "**/.DS_Store",
    "**/Thumbs.db",
    "**/logs/**",
    "**/tmp/**",
    "**/temp/**",
    "**/*.log",
    "**/coverage/**",
    "**/.nyc_output/**",
];

/// `Ignore.PATTERNS`: FILES followed by FOLDERS.
pub fn patterns() -> Vec<String> {
    FILES
        .iter()
        .chain(FOLDERS)
        .map(|item| (*item).to_string())
        .collect()
}

/// `Ignore.match(filepath, { extra, whitelist })`.
pub fn ignore_match(filepath: &str, extra: &[String], whitelist: &[String]) -> bool {
    if whitelist
        .iter()
        .any(|pattern| glob_match(pattern, filepath))
    {
        return false;
    }
    if filepath
        .split(['/', '\\'])
        .any(|part| FOLDERS.contains(&part))
    {
        return true;
    }
    FILES
        .iter()
        .map(|pattern| (*pattern).to_string())
        .chain(extra.iter().cloned())
        .any(|pattern| glob_match(&pattern, filepath))
}

const DARWIN_HOME: &[&str] = &[
    "Music",
    "Pictures",
    "Movies",
    "Downloads",
    "Desktop",
    "Documents",
    "Public",
    "Applications",
    "Library",
];

const DARWIN_LIBRARY: &[&str] = &[
    "Application Support/AddressBook",
    "Calendars",
    "Mail",
    "Messages",
    "Safari",
    "Cookies",
    "Application Support/com.apple.TCC",
    "PersonalizationPortrait",
    "Metadata/CoreSpotlight",
    "Suggestions",
];

const DARWIN_ROOT: &[&str] = &[
    "/.DocumentRevisions-V100",
    "/.Spotlight-V100",
    "/.Trashes",
    "/.fseventsd",
];

const WIN32_HOME: &[&str] = &[
    "AppData",
    "Downloads",
    "Desktop",
    "Documents",
    "Pictures",
    "Music",
    "Videos",
    "OneDrive",
];

/// `Protected.names()`: home directory basenames to skip when scanning home.
pub fn protected_names() -> BTreeSet<&'static str> {
    if cfg!(target_os = "macos") {
        return DARWIN_HOME.iter().copied().collect();
    }
    if cfg!(windows) {
        return WIN32_HOME.iter().copied().collect();
    }
    BTreeSet::new()
}

/// `Protected.paths()` for the current user's home directory.
pub fn protected_paths() -> Vec<PathBuf> {
    protected_paths_for(&dirs::home_dir().unwrap_or_default())
}

/// `Protected.paths()` with an explicit home (testable without touching the real home).
pub fn protected_paths_for(home: &Path) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        return DARWIN_HOME
            .iter()
            .map(|name| home.join(name))
            .chain(
                DARWIN_LIBRARY
                    .iter()
                    .map(|name| home.join("Library").join(name)),
            )
            .chain(DARWIN_ROOT.iter().map(PathBuf::from))
            .collect();
    }
    if cfg!(windows) {
        return WIN32_HOME.iter().map(|name| home.join(name)).collect();
    }
    Vec::new()
}

/// Watcher `protecteds(dir)`: protected paths strictly below `dir`.
pub fn protected_below(dir: &Path, protected: &[PathBuf]) -> Vec<PathBuf> {
    protected
        .iter()
        .filter(|item| {
            let relative = opencode_platform_api::path::relative(dir, item);
            !relative.is_empty()
                && !relative.starts_with("..")
                && !Path::new(&relative).is_absolute()
        })
        .cloned()
        .collect()
}
