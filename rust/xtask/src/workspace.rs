//! Purpose: shared workspace location helpers for xtask tasks (rust root discovery, path
//! normalization to the header grammar's `rust/<root>/` form).
//! Scope: build-time tooling.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! `-- walkdir::WalkDir::new
//! Used by:
//! rust/xtask/
//! |-- src/edges.rs
//! |-- src/fixtures.rs
//! |-- src/headers.rs
//! |-- src/inventory.rs
//! |-- src/lib.rs
//! |-- src/size.rs
//! `-- tests/gates.rs
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

/// Collect every Rust source, including tests, benches and build scripts, without target trees.
pub fn sources(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for directory in ["crates", "bins", "xtask"] {
        for entry in walkdir::WalkDir::new(root.join(directory))
            .into_iter()
            .filter_entry(|entry| !matches!(entry.file_name().to_str(), Some("target" | ".git")))
        {
            let entry = entry?;
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "rs")
            {
                files.push(entry.into_path());
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn finish(gate: &str, errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        println!("xtask {gate}: passed");
        return Ok(());
    }
    bail!(
        "xtask {gate}: {} violation(s)\n{}",
        errors.len(),
        errors.join("\n")
    )
}

/// Splits a `fix`-style flag out of task args.
pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}
