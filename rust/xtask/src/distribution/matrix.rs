//! Purpose: the release target matrix: baseline asset naming (`opencode-<os>-<arch>[-baseline][-musl]`),
//! Rust target triples, archive formats and the D13 executable/blocked status per row.
//! Scope: pure data plus serialization of `rust/distribution/release-matrix.json`.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- serde_json::to_string_pretty
//! Used by:
//! rust/xtask/
//! |-- src/distribution/build.rs
//! |-- src/distribution/mod.rs
//! |-- src/distribution/tests.rs
//! `-- src/distribution/verify.rs
//! Invariants: row order and names mirror `packages/opencode/script/build.ts` allTargets, linux
//! archives are tar.gz and every other OS is zip, only D13-reachable rows are `executable`.
//! Contract tests: xtask/src/distribution/tests.rs

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "opencode-release-matrix/v1";
pub const PRODUCT: &str = "opencode";

/// Static description of one baseline release row.
pub struct TargetSpec {
    pub os: &'static str,
    pub arch: &'static str,
    pub musl: bool,
    pub baseline: bool,
    pub triple: &'static str,
    pub rustflags: Option<&'static str>,
    pub ci_runner: &'static str,
    /// `Some(runner)` when D13 says agents can execute this row.
    pub local_runner: Option<&'static str>,
}

const X64_V3: Option<&str> = Some("-C target-cpu=x86-64-v3");
const X64_BASE: Option<&str> = Some("-C target-cpu=x86-64");

/// Same order as `allTargets` in packages/opencode/script/build.ts.
pub const TARGETS: &[TargetSpec] = &[
    spec(
        "linux",
        "arm64",
        false,
        false,
        "aarch64-unknown-linux-gnu",
        None,
        "ubuntu-24.04-arm",
        Some("docker linux/arm64"),
    ),
    spec(
        "linux",
        "x64",
        false,
        false,
        "x86_64-unknown-linux-gnu",
        X64_V3,
        "ubuntu-24.04",
        None,
    ),
    spec(
        "linux",
        "x64",
        false,
        true,
        "x86_64-unknown-linux-gnu",
        X64_BASE,
        "ubuntu-24.04",
        None,
    ),
    spec(
        "linux",
        "arm64",
        true,
        false,
        "aarch64-unknown-linux-musl",
        None,
        "ubuntu-24.04-arm",
        Some("docker linux/arm64"),
    ),
    spec(
        "linux",
        "x64",
        true,
        false,
        "x86_64-unknown-linux-musl",
        X64_V3,
        "ubuntu-24.04",
        None,
    ),
    spec(
        "linux",
        "x64",
        true,
        true,
        "x86_64-unknown-linux-musl",
        X64_BASE,
        "ubuntu-24.04",
        None,
    ),
    spec(
        "darwin",
        "arm64",
        false,
        false,
        "aarch64-apple-darwin",
        None,
        "macos-15",
        Some("local aarch64-apple-darwin"),
    ),
    spec(
        "darwin",
        "x64",
        false,
        false,
        "x86_64-apple-darwin",
        X64_V3,
        "macos-15-intel",
        None,
    ),
    spec(
        "darwin",
        "x64",
        false,
        true,
        "x86_64-apple-darwin",
        X64_BASE,
        "macos-15-intel",
        None,
    ),
    spec(
        "windows",
        "arm64",
        false,
        false,
        "aarch64-pc-windows-msvc",
        None,
        "windows-11-arm",
        None,
    ),
    spec(
        "windows",
        "x64",
        false,
        false,
        "x86_64-pc-windows-msvc",
        X64_V3,
        "windows-2025",
        None,
    ),
    spec(
        "windows",
        "x64",
        false,
        true,
        "x86_64-pc-windows-msvc",
        X64_BASE,
        "windows-2025",
        None,
    ),
];

#[allow(clippy::too_many_arguments)]
const fn spec(
    os: &'static str,
    arch: &'static str,
    musl: bool,
    baseline: bool,
    triple: &'static str,
    rustflags: Option<&'static str>,
    ci_runner: &'static str,
    local_runner: Option<&'static str>,
) -> TargetSpec {
    TargetSpec {
        os,
        arch,
        musl,
        baseline,
        triple,
        rustflags,
        ci_runner,
        local_runner,
    }
}

impl TargetSpec {
    /// Mirrors build.ts: `[pkg.name, os, arch, baseline?, abi?].join("-")`.
    pub fn asset(&self) -> String {
        let mut name = format!("{PRODUCT}-{}-{}", self.os, self.arch);
        if self.baseline {
            name.push_str("-baseline");
        }
        if self.musl {
            name.push_str("-musl");
        }
        name
    }

    /// build.ts: linux keys become `.tar.gz`, everything else `.zip`.
    pub fn archive(&self) -> String {
        let ext = if self.os == "linux" { "tar.gz" } else { "zip" };
        format!("{}.{ext}", self.asset())
    }

    pub fn executable(&self) -> String {
        if self.os == "windows" {
            format!("{PRODUCT}.exe")
        } else {
            PRODUCT.to_string()
        }
    }
}

/// Look up a row by asset name (`opencode-linux-arm64`), short name (`linux-arm64`) or triple.
/// A bare triple resolves to the non-baseline row.
pub fn find(key: &str) -> Option<&'static TargetSpec> {
    let asset = if key.starts_with(&format!("{PRODUCT}-")) {
        key.to_string()
    } else {
        format!("{PRODUCT}-{key}")
    };
    TARGETS
        .iter()
        .find(|t| t.asset() == asset)
        .or_else(|| TARGETS.iter().find(|t| t.triple == key && !t.baseline))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: String,
    pub decision: String,
    pub product: String,
    pub targets: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub asset: String,
    pub archive: String,
    pub executable: String,
    pub os: String,
    pub arch: String,
    pub libc: Option<String>,
    pub baseline: bool,
    pub rust_target: String,
    pub rustflags: Option<String>,
    pub ci_runner: String,
    pub status: String,
    pub local_runner: Option<String>,
    pub reason: Option<String>,
}

/// The manifest as defined in code. The checked-in JSON must equal this byte for byte.
pub fn manifest() -> Manifest {
    Manifest {
        schema: SCHEMA.into(),
        decision: "rust/contracts/phase-90/decisions.md#D13".into(),
        product: PRODUCT.into(),
        targets: TARGETS
            .iter()
            .map(|t| Row {
                asset: t.asset(),
                archive: t.archive(),
                executable: t.executable(),
                os: t.os.into(),
                arch: t.arch.into(),
                libc: (t.os == "linux").then(|| if t.musl { "musl" } else { "glibc" }.into()),
                baseline: t.baseline,
                rust_target: t.triple.into(),
                rustflags: t.rustflags.map(Into::into),
                ci_runner: t.ci_runner.into(),
                status: if t.local_runner.is_some() {
                    "executable"
                } else {
                    "blocked"
                }
                .into(),
                local_runner: t.local_runner.map(Into::into),
                reason: t.local_runner.is_none().then(|| {
                    "runner unavailable (D13), workflow ready in .github/workflows/rust.yml".into()
                }),
            })
            .collect(),
    }
}

pub fn render(manifest: &Manifest) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("manifest serializes");
    text.push('\n');
    text
}
