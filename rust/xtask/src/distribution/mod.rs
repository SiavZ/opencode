//! Purpose: `cargo xtask distribution <build|checksums|sbom|verify|matrix>`: build release archives
//! named like the baseline GitHub release assets, write SHA256SUMS, a CycloneDX SBOM generated from
//! `cargo metadata` + Cargo.lock, and verify the release-matrix manifest and checksums.
//! Scope: build-time tooling only. Never publishes, uploads, signs or touches user data.
//! Uses:
//! rust/xtask/
//! |-- src/distribution/archive.rs
//! |-- src/distribution/build.rs
//! |-- src/distribution/matrix.rs
//! |-- src/distribution/sbom.rs
//! |-- src/distribution/tests.rs
//! `-- src/distribution/verify.rs
//! External:
//! `-- anyhow::{Context, Result}
//! Used by:
//! rust/xtask/
//! `-- src/main.rs
//! Invariants: outputs are byte-deterministic for the same inputs (fixed archive mtimes/owners,
//! sorted checksums, no SBOM timestamp), failures are typed `DistError` values.
//! Contract tests: xtask/src/distribution/tests.rs

pub mod archive;
pub mod build;
pub mod matrix;
pub mod sbom;
#[cfg(test)]
mod tests;
pub mod verify;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const USAGE: &str = "cargo xtask distribution <task> [options]

tasks:
  build      [--package P] [--bin B] [--target ROW] [--out DIR] [--no-sbom]
             cargo build --release, archive as <asset>.tar.gz|zip, SBOM, SHA256SUMS
  checksums  --dir DIR                 write DIR/SHA256SUMS for every archive and SBOM
  sbom       [--package P] [--target ROW] [--artifact FILE] [--out FILE]
  verify     [--manifest FILE] [--dir DIR]
             check the manifest equals the code matrix, and DIR/SHA256SUMS + archives
  matrix     [--write]                 print or rewrite distribution/release-matrix.json

ROW is an asset (opencode-linux-arm64-musl), short name (darwin-arm64) or Rust triple.
--package defaults to `opencode` (rust/bins/opencode).";

/// Typed failures. Callers and tests match on these, not on strings.
#[derive(Debug)]
pub enum DistError {
    Usage(String),
    UnknownTarget(String),
    PackageMissing(String),
    BinMissing {
        package: String,
        bin: String,
    },
    TargetNotInstalled(String),
    BuildFailed {
        package: String,
        status: String,
    },
    ManifestDrift {
        path: PathBuf,
        detail: String,
    },
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    ChecksumMissing(String),
    UnlistedArtifact(String),
    ArchiveContents {
        file: String,
        detail: String,
    },
}

impl fmt::Display for DistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) => write!(f, "usage: {m}\n\n{USAGE}"),
            Self::UnknownTarget(t) => write!(
                f,
                "unknown release target `{t}` (see distribution/release-matrix.json)"
            ),
            Self::PackageMissing(p) => write!(
                f,
                "workspace package `{p}` does not exist; pass --package <existing bin package>"
            ),
            Self::BinMissing { package, bin } => {
                write!(f, "package `{package}` has no binary target `{bin}`")
            }
            Self::TargetNotInstalled(t) => write!(
                f,
                "rust target `{t}` is not installed (rustup target add {t}) or no runner is available (D13)"
            ),
            Self::BuildFailed { package, status } => {
                write!(f, "cargo build for `{package}` failed: {status}")
            }
            Self::ManifestDrift { path, detail } => write!(
                f,
                "{} drifted from the code matrix: {detail}; run `cargo xtask distribution matrix --write`",
                path.display()
            ),
            Self::ChecksumMismatch {
                file,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch for {file}: expected {expected}, got {actual}"
            ),
            Self::ChecksumMissing(file) => {
                write!(f, "{file} is listed in SHA256SUMS but missing on disk")
            }
            Self::UnlistedArtifact(file) => write!(
                f,
                "{file} is not a release-matrix archive or is not covered by SHA256SUMS"
            ),
            Self::ArchiveContents { file, detail } => write!(f, "{file}: {detail}"),
        }
    }
}

impl std::error::Error for DistError {}

pub fn run(args: &[String]) -> Result<()> {
    let (task, rest) = args
        .split_first()
        .map(|(t, r)| (t.as_str(), r))
        .unwrap_or(("help", &[]));
    let flags = parse_flags(rest)?;
    let root = workspace_root();
    match task {
        "build" => {
            let report = build::run(&root, &build::Options::from_flags(&root, &flags)?)?;
            println!("{report}");
            Ok(())
        }
        "checksums" => {
            let dir = PathBuf::from(
                flags
                    .get("dir")
                    .ok_or_else(|| DistError::Usage("checksums needs --dir".into()))?,
            );
            let sums = verify::write_checksums(&dir)?;
            print!("{sums}");
            Ok(())
        }
        "sbom" => {
            let package = flags.get("package").map_or("opencode", String::as_str);
            let target = flags
                .get("target")
                .map(|t| matrix::find(t).ok_or_else(|| DistError::UnknownTarget(t.clone())))
                .transpose()?;
            let artifact = flags.get("artifact").map(PathBuf::from);
            let doc = sbom::generate(
                &root,
                package,
                target.map(|t| t.triple),
                artifact.as_deref(),
            )?;
            let text = sbom::render(&doc);
            match flags.get("out") {
                Some(out) => std::fs::write(out, text).with_context(|| format!("writing {out}"))?,
                None => print!("{text}"),
            }
            Ok(())
        }
        "verify" => {
            let manifest = flags.get("manifest").map_or_else(
                || root.join("distribution/release-matrix.json"),
                PathBuf::from,
            );
            let report = verify::run(&manifest, flags.get("dir").map(Path::new))?;
            println!("{report}");
            Ok(())
        }
        "matrix" => {
            let text = matrix::render(&matrix::manifest());
            if flags.contains_key("write") {
                let path = root.join("distribution/release-matrix.json");
                std::fs::write(&path, text)
                    .with_context(|| format!("writing {}", path.display()))?;
                println!("wrote {}", path.display());
                return Ok(());
            }
            print!("{text}");
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(DistError::Usage(format!("unknown distribution task `{other}`")).into()),
    }
}

/// `--key value` and bare `--switch` flags. Switches map to an empty string.
pub fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, DistError> {
    let mut flags = BTreeMap::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let key = arg
            .strip_prefix("--")
            .ok_or_else(|| DistError::Usage(format!("unexpected argument `{arg}`")))?;
        let value = iter
            .next_if(|next| !next.starts_with("--"))
            .cloned()
            .unwrap_or_default();
        flags.insert(key.to_string(), value);
    }
    Ok(flags)
}

/// `rust/`, the Cargo workspace root (xtask lives at rust/xtask).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .to_path_buf()
}
