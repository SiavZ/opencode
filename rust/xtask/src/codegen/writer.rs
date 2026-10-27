//! Purpose: write only validated manifest-owned generated files and reject unsafe output paths.
//! Scope: filesystem boundary for Rust client code generation.
//! Uses:
//! `-- rust/xtask/src/codegen/mod.rs
//! External:
//! |-- anyhow
//! `-- serde_json
//! Used by:
//! `-- rust/xtask/src/codegen/mod.rs
//! Invariants: symlinks and case-colliding paths fail before writes; stale removals require prior ownership.
//! Contract tests: xtask/tests/codegen.rs
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
const MANIFEST: &str = ".rust-codegen.json";

pub(super) fn write(root: &Path, files: &BTreeMap<String, String>, check: bool) -> Result<()> {
    if root
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("generated root is a symlink");
    }
    validate(root, MANIFEST)?;
    if files.contains_key(MANIFEST) {
        bail!("generated output cannot replace its ownership manifest");
    }
    let owned = if root.join(MANIFEST).exists() {
        serde_json::from_slice::<Vec<String>>(&std::fs::read(root.join(MANIFEST))?)?
    } else {
        vec![]
    };
    let mut cases = BTreeSet::new();
    for path in files.keys().chain(&owned) {
        validate(root, path)?;
    }
    for path in files.keys() {
        if !cases.insert(path.to_ascii_lowercase()) {
            bail!("case-colliding generated path: {path}");
        }
    }
    let manifest = serde_json::to_string_pretty(&files.keys().collect::<Vec<_>>())? + "\n";
    let mut differences = vec![];
    for (path, content) in files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .chain([(MANIFEST, manifest.as_str())])
    {
        if path != MANIFEST && root.join(path).exists() && !owned.iter().any(|entry| entry == path)
        {
            bail!("refusing to replace unowned file: {path}");
        }
        if std::fs::read_to_string(root.join(path)).ok().as_deref() == Some(content) {
            continue;
        }
        if check {
            differences.push(path.to_owned());
            continue;
        }
        std::fs::create_dir_all(
            root.join(path)
                .parent()
                .context("generated file has no parent")?,
        )?;
        std::fs::write(root.join(path), content)?;
    }
    for path in owned.iter().filter(|path| !files.contains_key(*path)) {
        if !root.join(path).exists() {
            continue;
        }
        if check {
            differences.push(path.clone());
        } else {
            std::fs::remove_file(root.join(path))?;
        }
    }
    if !differences.is_empty() {
        bail!("generated files differ: {}", differences.join(", "));
    }
    Ok(())
}
fn validate(root: &Path, value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || Path::new(value)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("unsafe generated path: {value}");
    }
    let mut current = std::path::PathBuf::new();
    for component in root.components().chain(Path::new(value).components()) {
        current.push(component);
        if current
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("generated path crosses symlink: {}", current.display());
        }
    }
    Ok(())
}
