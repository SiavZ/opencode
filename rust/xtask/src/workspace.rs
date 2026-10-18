//! Purpose: shared workspace location helpers for xtask tasks (rust root discovery, path
//! normalization to the header grammar's `rust/<root>/` form).
//! Scope: build-time tooling.
//! Uses:
//! `-- none
//! External:
//! `-- anyhow::{Context, Result}
//! Used by:
//! rust/xtask/
//! `-- src/lib.rs
//! Invariants: paths are reported with `/` separators relative to the repository root.
//! Contract tests: xtask/tests/

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The `rust/` workspace root: `$XTASK_RUST_ROOT`, else the parent of this crate's manifest dir.
pub fn rust_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("XTASK_RUST_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask manifest has no parent directory")
}

/// Repository root (parent of `rust/`), where the TypeScript baseline lives.
pub fn repo_root(rust_root: &Path) -> Result<PathBuf> {
    match rust_root.parent() {
        Some(parent) => Ok(parent.to_path_buf()),
        None => bail!("rust root {} has no parent", rust_root.display()),
    }
}

/// `rust/crates/schema/src/lib.rs` style display path for a file under `rust_root`.
pub fn display_path(rust_root: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(rust_root).unwrap_or(file);
    let joined = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    format!("rust/{joined}")
}

/// Splits a `fix`-style flag out of task args.
pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}
