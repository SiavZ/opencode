//! Purpose: `distribution build`: resolve the package/bin, run `cargo build --release` for one
//! release-matrix row, archive the executable under the baseline asset name, write the SBOM and
//! SHA256SUMS into the output directory.
//! Scope: spawns cargo and rustc, writes only under `--out` (default `<target-dir>/distribution`).
//! Uses:
//! rust/xtask/
//! |-- src/distribution/archive.rs
//! |-- src/distribution/matrix.rs
//! |-- src/distribution/mod.rs
//! |-- src/distribution/sbom.rs
//! `-- src/distribution/verify.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::ensure
//! |-- archive::write
//! |-- matrix::PRODUCT
//! |-- matrix::find
//! |-- sbom::generate
//! |-- sbom::render
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- str::to_string
//! `-- verify::write_checksums
//! Used by:
//! rust/xtask/
//! |-- src/distribution/mod.rs
//! `-- src/distribution/tests.rs
//! Invariants: a missing package/bin/target is a typed `DistError` before any compilation starts.
//! Builds always pass `--locked`. Row rustflags (x64 v3 vs baseline) are applied via
//! `CARGO_ENCODED_RUSTFLAGS` only when building that row.
//! Contract tests: xtask/src/distribution/tests.rs

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use super::matrix::{self, TargetSpec};
use super::{DistError, archive, sbom, verify};

pub struct Options {
    pub package: String,
    pub bin: String,
    pub target: &'static TargetSpec,
    pub out: PathBuf,
    pub sbom: bool,
}

impl Options {
    pub fn from_flags(root: &Path, flags: &BTreeMap<String, String>) -> Result<Self, DistError> {
        let package = flags
            .get("package")
            .cloned()
            .unwrap_or_else(|| matrix::PRODUCT.to_string());
        let bin = flags.get("bin").cloned().unwrap_or_else(|| package.clone());
        let target = match flags.get("target") {
            Some(key) => matrix::find(key).ok_or_else(|| DistError::UnknownTarget(key.clone()))?,
            None => host_row()?,
        };
        let out = flags
            .get("out")
            .map_or_else(|| target_dir(root).join("distribution"), PathBuf::from);
        Ok(Self {
            package,
            bin,
            target,
            out,
            sbom: !flags.contains_key("no-sbom"),
        })
    }
}

pub fn run(root: &Path, opts: &Options) -> Result<String> {
    let metadata = metadata(root)?;
    require_bin(&metadata, &opts.package, &opts.bin)?;
    require_target(opts.target.triple)?;

    let mut cmd = Command::new(cargo());
    cmd.current_dir(root).args([
        "build",
        "--release",
        "--locked",
        "-p",
        &opts.package,
        "--bin",
        &opts.bin,
        "--target",
        opts.target.triple,
    ]);
    if let Some(flags) = opts.target.rustflags {
        cmd.env("CARGO_ENCODED_RUSTFLAGS", flags.replace(' ', "\x1f"));
    }
    let status = cmd.status().context("spawning cargo build")?;
    if !status.success() {
        return Err(DistError::BuildFailed {
            package: opts.package.clone(),
            status: status.to_string(),
        }
        .into());
    }

    let exe = if opts.target.os == "windows" {
        format!("{}.exe", opts.bin)
    } else {
        opts.bin.clone()
    };
    let built = target_dir(root)
        .join(opts.target.triple)
        .join("release")
        .join(&exe);
    std::fs::create_dir_all(&opts.out)
        .with_context(|| format!("creating {}", opts.out.display()))?;
    let archive_path = opts.out.join(opts.target.archive());
    // The archive entry is always the product executable name, as in the baseline release.
    archive::write(&built, &opts.target.executable(), &archive_path)?;

    let mut report = String::new();
    writeln!(
        report,
        "built {} ({}) for {}",
        opts.bin, opts.package, opts.target.triple
    )?;
    writeln!(report, "archive {}", archive_path.display())?;
    if opts.sbom {
        let doc = sbom::generate(root, &opts.package, Some(opts.target.triple), Some(&built))?;
        let path = opts.out.join(format!("{}.cdx.json", opts.target.asset()));
        std::fs::write(&path, sbom::render(&doc))
            .with_context(|| format!("writing {}", path.display()))?;
        writeln!(
            report,
            "sbom {} ({} components)",
            path.display(),
            doc["components"].as_array().map_or(0, Vec::len)
        )?;
    }
    let sums = verify::write_checksums(&opts.out)?;
    write!(report, "SHA256SUMS\n{sums}")?;
    Ok(report)
}

/// The release-matrix row for the machine running xtask.
pub fn host_row() -> Result<&'static TargetSpec, DistError> {
    let host =
        rustc_host().map_err(|e| DistError::Usage(format!("cannot determine host triple: {e}")))?;
    matrix::find(&host).ok_or(DistError::UnknownTarget(host))
}

pub fn require_bin(metadata: &Value, package: &str, bin: &str) -> Result<(), DistError> {
    let members: Vec<&str> = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let pkg = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == package && p["id"].as_str().is_some_and(|id| members.contains(&id)))
        .ok_or_else(|| DistError::PackageMissing(package.to_string()))?;
    let has_bin = pkg["targets"].as_array().into_iter().flatten().any(|t| {
        t["name"] == bin
            && t["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "bin"))
    });
    if has_bin {
        Ok(())
    } else {
        Err(DistError::BinMissing {
            package: package.to_string(),
            bin: bin.to_string(),
        })
    }
}

fn require_target(triple: &str) -> Result<(), DistError> {
    let host = rustc_host().unwrap_or_default();
    if host == triple {
        return Ok(());
    }
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok();
    let listed = installed.is_some_and(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim() == triple)
    });
    if listed {
        Ok(())
    } else {
        Err(DistError::TargetNotInstalled(triple.to_string()))
    }
}

fn metadata(root: &Path) -> Result<Value> {
    let output = Command::new(cargo())
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .context("running cargo metadata")?;
    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn rustc_host() -> Result<String> {
    let output = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("-vV")
        .output()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .context("rustc -vV printed no host line")
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}
