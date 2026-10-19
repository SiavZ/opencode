//! Purpose: SHA256SUMS writing and `distribution verify`: the checked-in release-matrix manifest
//! must equal the code matrix, every artifact in a dist directory must be listed with a matching
//! SHA-256, archive names must be release-matrix rows and each archive must hold exactly the
//! product executable.
//! Scope: reads/writes the given directory and manifest file only.
//! Uses:
//! rust/xtask/
//! |-- src/distribution/archive.rs
//! |-- src/distribution/matrix.rs
//! `-- src/distribution/mod.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- archive::list
//! |-- hex::encode
//! |-- matrix::Manifest
//! |-- matrix::manifest
//! |-- matrix::render
//! |-- serde_json::from_str
//! |-- sha2::Digest
//! `-- sha2::Sha256
//! Used by:
//! rust/xtask/
//! |-- src/distribution/build.rs
//! |-- src/distribution/mod.rs
//! `-- src/distribution/tests.rs
//! Invariants: SHA256SUMS uses the coreutils `sha256sum` text format (`<hex>  <name>`), sorted by
//! name, so `sha256sum -c SHA256SUMS` accepts it. Verification never rewrites files.
//! Contract tests: xtask/src/distribution/tests.rs

use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{DistError, archive, matrix};

pub const SUMS: &str = "SHA256SUMS";

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Release artifacts in `dir`: archives and SBOMs, sorted by name.
pub fn artifacts(dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tar.gz") || n.ends_with(".zip") || n.ends_with(".cdx.json"))
        .collect();
    names.sort();
    Ok(names)
}

pub fn write_checksums(dir: &Path) -> Result<String> {
    let mut text = String::new();
    for name in artifacts(dir)? {
        writeln!(text, "{}  {name}", sha256_file(&dir.join(&name))?)?;
    }
    std::fs::write(dir.join(SUMS), &text)
        .with_context(|| format!("writing {}", dir.join(SUMS).display()))?;
    Ok(text)
}

pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once("  ").or_else(|| l.split_once(" *")))
        .map(|(sum, name)| (sum.trim().to_lowercase(), name.trim().to_string()))
        .collect()
}

pub fn run(manifest_path: &Path, dir: Option<&Path>) -> Result<String> {
    let mut report = String::new();
    check_manifest(manifest_path)?;
    let manifest = matrix::manifest();
    let executable = manifest
        .targets
        .iter()
        .filter(|r| r.status == "executable")
        .count();
    writeln!(
        report,
        "manifest {} ok: {} targets, {executable} executable, {} blocked (D13)",
        manifest_path.display(),
        manifest.targets.len(),
        manifest.targets.len() - executable
    )?;
    if let Some(dir) = dir {
        let checked = check_dir(dir)?;
        writeln!(
            report,
            "{} ok: {checked} artifacts match SHA256SUMS",
            dir.display()
        )?;
    }
    Ok(report)
}

pub fn check_manifest(path: &Path) -> Result<(), DistError> {
    let on_disk = std::fs::read_to_string(path).map_err(|e| DistError::ManifestDrift {
        path: path.to_path_buf(),
        detail: format!("unreadable: {e}"),
    })?;
    let parsed: matrix::Manifest =
        serde_json::from_str(&on_disk).map_err(|e| DistError::ManifestDrift {
            path: path.to_path_buf(),
            detail: format!("invalid JSON: {e}"),
        })?;
    let expected = matrix::manifest();
    if parsed != expected {
        let detail = expected
            .targets
            .iter()
            .zip(
                parsed
                    .targets
                    .iter()
                    .map(Some)
                    .chain(std::iter::repeat(None)),
            )
            .find(|(want, got)| Some(*want) != *got)
            .map_or_else(
                || "row count or header differs".to_string(),
                |(want, _)| format!("row {} differs", want.asset),
            );
        return Err(DistError::ManifestDrift {
            path: path.to_path_buf(),
            detail,
        });
    }
    if on_disk != matrix::render(&expected) {
        return Err(DistError::ManifestDrift {
            path: path.to_path_buf(),
            detail: "formatting differs".into(),
        });
    }
    Ok(())
}

/// Returns the number of verified artifacts.
pub fn check_dir(dir: &Path) -> Result<usize> {
    let sums_path = dir.join(SUMS);
    let text = std::fs::read_to_string(&sums_path)
        .map_err(|_| DistError::ChecksumMissing(SUMS.to_string()))?;
    let sums = parse_checksums(&text);
    for (expected, name) in &sums {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(DistError::ChecksumMissing(name.clone()).into());
        }
        let actual = sha256_file(&path)?;
        if &actual != expected {
            return Err(DistError::ChecksumMismatch {
                file: name.clone(),
                expected: expected.clone(),
                actual,
            }
            .into());
        }
    }
    let rows = matrix::manifest().targets;
    for name in artifacts(dir)? {
        if !sums.iter().any(|(_, n)| *n == name) {
            return Err(DistError::UnlistedArtifact(name).into());
        }
        if name.ends_with(".cdx.json") {
            continue;
        }
        let row = rows
            .iter()
            .find(|r| r.archive == name)
            .ok_or_else(|| DistError::UnlistedArtifact(name.clone()))?;
        let entries = archive::list(&dir.join(&name))?;
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        if names != [row.executable.as_str()] || entries[0].1.is_empty() {
            return Err(DistError::ArchiveContents {
                file: name,
                detail: format!("expected only `{}`, found {names:?}", row.executable),
            }
            .into());
        }
    }
    Ok(sums.len())
}
