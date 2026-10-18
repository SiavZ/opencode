//! Purpose: ripgrep executable resolution (packages/core/src/ripgrep/binary.ts): `rg` on PATH (with
//! the managed bin directory appended), then the managed `<bin>/rg`, then a download port.
//! Scope: filesystem probing and a cached result. Downloading is delegated to a port.
//! Uses:
//! `-- none
//! External:
//! `-- which::which_in
//! Used by:
//! rust/crates/
//! |-- tools/src/search/mod.rs
//! `-- tools/src/search/ripgrep.rs
//! Invariants: resolution order matches the baseline; the first success is cached for the process.
//! Contract tests: tools/tests/search_ripgrep.rs

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::sync::OnceCell;

pub const VERSION: &str = "15.1.0";

/// Release asset for the current platform (`PLATFORM` table), or `None` when unsupported.
pub fn platform_asset(arch: &str, os: &str) -> Option<(&'static str, &'static str)> {
    Some(match (arch, os) {
        ("aarch64", "macos") => ("aarch64-apple-darwin", "tar.gz"),
        ("aarch64", "linux") => ("aarch64-unknown-linux-gnu", "tar.gz"),
        ("x86_64", "macos") => ("x86_64-apple-darwin", "tar.gz"),
        ("x86_64", "linux") => ("x86_64-unknown-linux-musl", "tar.gz"),
        ("aarch64", "windows") => ("aarch64-pc-windows-msvc", "zip"),
        ("x86", "windows") => ("i686-pc-windows-msvc", "zip"),
        ("x86_64", "windows") => ("x86_64-pc-windows-msvc", "zip"),
        _ => return None,
    })
}

/// Download URL for a release asset.
pub fn download_url(platform: &str, extension: &str) -> String {
    format!(
        "https://github.com/BurntSushi/ripgrep/releases/download/{VERSION}/ripgrep-{VERSION}-{platform}.{extension}"
    )
}

/// Installs ripgrep into `target` when no local executable exists (network + archive extraction).
#[async_trait]
pub trait RipgrepInstaller: Send + Sync + std::fmt::Debug {
    async fn install(&self, url: &str, target: &Path) -> Result<(), String>;
}

/// Installer used when no network installer is composed: a typed, tested refusal.
#[derive(Debug, Default)]
pub struct NoInstaller;

#[async_trait]
impl RipgrepInstaller for NoInstaller {
    async fn install(&self, url: &str, _target: &Path) -> Result<(), String> {
        Err(format!(
            "ripgrep is not installed and downloading is not ported yet: {url}"
        ))
    }
}

#[derive(Debug)]
pub struct RipgrepBinary {
    bin: PathBuf,
    path: Option<String>,
    installer: std::sync::Arc<dyn RipgrepInstaller>,
    cached: OnceCell<PathBuf>,
}

fn executable() -> &'static str {
    if cfg!(windows) { "rg.exe" } else { "rg" }
}

impl RipgrepBinary {
    /// `bin` is `Global.Path.bin`; `path` overrides the inherited `PATH` (tests).
    pub fn new(
        bin: impl Into<PathBuf>,
        path: Option<String>,
        installer: std::sync::Arc<dyn RipgrepInstaller>,
    ) -> Self {
        Self {
            bin: bin.into(),
            path,
            installer,
            cached: OnceCell::new(),
        }
    }

    pub async fn filepath(&self) -> Result<PathBuf, String> {
        self.cached
            .get_or_try_init(|| self.resolve())
            .await
            .cloned()
    }

    async fn resolve(&self) -> Result<PathBuf, String> {
        let base = self
            .path
            .clone()
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        let separator = if cfg!(windows) { ";" } else { ":" };
        let full = if base.is_empty() {
            self.bin.to_string_lossy().into_owned()
        } else {
            format!("{base}{separator}{}", self.bin.to_string_lossy())
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        if let Ok(system) = which::which_in(executable(), Some(full), cwd)
            && system.is_file()
        {
            return Ok(system);
        }
        let target = self.bin.join(executable());
        if target.is_file() {
            return Ok(target);
        }
        let (platform, extension) = platform_asset(std::env::consts::ARCH, std::env::consts::OS)
            .ok_or_else(|| {
                format!(
                    "unsupported platform for ripgrep: {}-{}",
                    std::env::consts::ARCH,
                    std::env::consts::OS
                )
            })?;
        self.installer
            .install(&download_url(platform, extension), &target)
            .await?;
        Ok(target)
    }
}
