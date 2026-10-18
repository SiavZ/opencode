//! Purpose: node-glob style scans for `FSUtil.glob`/`globUp` and config discovery
//! (`packages/core/src/util/glob.ts`): brace sets, `**`, optional dot files, optional symlink
//! following, files only unless directories are requested.
//! Scope: blocking directory walk, callers run it on the blocking pool.
//! Uses:
//! rust/crates/
//! `-- platform-native/src/fs/error.rs
//! External:
//! |-- globset::GlobBuilder
//! `-- walkdir::WalkDir
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! `-- platform-native/src/fs/native.rs
//! Invariants: `*` never crosses `/`; without `dot`, components starting with `.` match only when
//! the pattern component itself starts with `.`; results are sorted for determinism (node-glob
//! order is unspecified); symlink cycles are skipped, not fatal.
//! Contract tests: platform-native/tests/fs_util.rs

use super::error::io_error;
use globset::{GlobBuilder, GlobMatcher};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use std::path::{Path, PathBuf};

/// `Glob.Options` from the baseline (`include: "file" | "all"` maps to `include_dirs`).
#[derive(Debug, Clone, Default)]
pub struct GlobOptions {
    pub cwd: PathBuf,
    pub absolute: bool,
    pub include_dirs: bool,
    pub dot: bool,
    pub follow_symlinks: bool,
}

/// `minimatch(filepath, pattern, { dot: true })` used by `Glob.match` and ignore rules.
pub fn glob_match(pattern: &str, filepath: &str) -> bool {
    compile(pattern).is_some_and(|matcher| matcher.is_match(filepath))
}

pub(crate) fn compile(pattern: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher())
}

/// Scan `pattern` below `options.cwd` (or from the pattern root when the pattern is absolute).
pub fn scan(pattern: &str, options: &GlobOptions) -> Result<Vec<String>, PlatformError> {
    let pattern = pattern.replace('\\', "/");
    let absolute_pattern = Path::new(&pattern).is_absolute();
    let base = if absolute_pattern {
        PathBuf::from("/")
    } else {
        options.cwd.clone()
    };
    let relative_pattern = pattern.trim_start_matches('/').to_string();
    let matcher = compile(&relative_pattern).ok_or_else(|| {
        PlatformError::new(
            PlatformErrorKind::InvalidInput,
            "glob",
            &pattern,
            "invalid glob pattern",
        )
    })?;
    let segments: Vec<&str> = relative_pattern.split('/').collect();
    let literal: Vec<&str> = segments
        .iter()
        .take_while(|segment| !is_magic(segment))
        .copied()
        .collect();
    let root = literal
        .iter()
        .fold(base.clone(), |dir, part| dir.join(part));
    let recursive = segments.iter().any(|segment| segment.contains("**"));
    let depth = segments.len() - literal.len();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let walker = walkdir::WalkDir::new(&root)
        .follow_links(options.follow_symlinks)
        .min_depth(0)
        .max_depth(if recursive { usize::MAX } else { depth });
    let mut out = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.loop_ancestor().is_some() => continue,
            Err(error) if error.depth() > 0 => continue,
            Err(error) => {
                let io = error
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("walk failed"));
                return Err(io_error("glob", &root, &io));
            }
        };
        let Ok(relative) = entry.path().strip_prefix(&base) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() || !matcher.is_match(&relative) {
            continue;
        }
        if !options.dot && hidden_mismatch(&relative, &segments) {
            continue;
        }
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|info| info.is_dir());
        if is_dir && !options.include_dirs {
            continue;
        }
        out.push(if options.absolute || absolute_pattern {
            entry.path().to_string_lossy().into_owned()
        } else {
            relative
        });
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn is_magic(segment: &str) -> bool {
    segment.contains(['*', '?', '[', '{'])
}

/// Node-glob without `dot`: a hidden path component must be matched by a pattern component that
/// explicitly starts with `.`.
fn hidden_mismatch(relative: &str, segments: &[&str]) -> bool {
    let explicit = segments.iter().any(|segment| segment.starts_with('.'));
    relative
        .split('/')
        .any(|part| part.starts_with('.') && part != "." && part != "..")
        && !explicit
}
