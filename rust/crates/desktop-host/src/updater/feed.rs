//! Purpose: fetch Tauri latest.json releases and stage only signature-verified artifacts.
//! Scope: native HTTP updater backend, installer handoff remains injected.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/updater/controller.rs
//! |-- desktop-host/src/updater/signature.rs
//! `-- desktop-host/src/updater/state.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- parking_lot::Mutex
//! |-- reqwest::Client
//! |-- semver::Version
//! |-- serde::Deserialize
//! |-- serde_json::from_slice
//! |-- tokio::{fs, sync::Mutex}
//! |-- url::Url
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/updater.rs
//! `-- desktop-host/tests/updater_feed.rs
//! Invariants: HTTPS except numeric loopback HTTP, no redirects, bounded reads, verification before disk writes.
//! Contract tests: desktop-host/tests/updater_feed.rs

use super::{CheckResult, SignatureVerifier, UpdaterBackend, UpdaterError};
use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use url::Url;

#[derive(Debug, Clone, Deserialize)]
pub struct FeedPlatform {
    pub url: String,
    pub signature: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedRelease {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, FeedPlatform>,
}

#[derive(Debug, Clone)]
pub struct FeedConfig {
    pub feed_url: Url,
    pub current_version: String,
    pub platform: String,
    pub staging_dir: PathBuf,
    pub timeout: Duration,
    pub max_feed_bytes: usize,
    pub max_artifact_bytes: usize,
}

impl FeedConfig {
    pub fn new(
        feed_url: Url,
        current_version: String,
        platform: String,
        staging_dir: PathBuf,
    ) -> Self {
        Self {
            feed_url,
            current_version,
            platform,
            staging_dir,
            timeout: Duration::from_secs(60),
            max_feed_bytes: 1024 * 1024,
            max_artifact_bytes: 256 * 1024 * 1024,
        }
    }
}

pub trait Installer: Send + Sync + 'static {
    fn install(&self, artifact: &Path, release: &FeedRelease) -> Result<(), UpdaterError>;
}

#[derive(Debug, Default)]
pub struct UnsupportedInstaller;

impl Installer for UnsupportedInstaller {
    fn install(&self, _: &Path, _: &FeedRelease) -> Result<(), UpdaterError> {
        Err(UpdaterError::Unsupported(
            "native updater install/relaunch is not yet ported",
        ))
    }
}

#[derive(Default)]
struct Selection {
    release: Option<FeedRelease>,
    staged: Option<PathBuf>,
}

pub struct FeedBackend {
    config: FeedConfig,
    client: reqwest::Client,
    verifier: Arc<dyn SignatureVerifier>,
    installer: Arc<dyn Installer>,
    selection: Mutex<Selection>,
    operation: tokio::sync::Mutex<()>,
}

impl FeedBackend {
    pub fn new(
        config: FeedConfig,
        verifier: Arc<dyn SignatureVerifier>,
        installer: Arc<dyn Installer>,
    ) -> Result<Self, UpdaterError> {
        validate_url(&config.feed_url)?;
        parse_version(&config.current_version)?;
        if config.timeout.is_zero() || config.max_feed_bytes == 0 || config.max_artifact_bytes == 0
        {
            return Err(UpdaterError::InvalidFeed(
                "timeout and byte limits must be positive".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(network)?;
        Ok(Self {
            config,
            client,
            verifier,
            installer,
            selection: Mutex::new(Selection::default()),
            operation: tokio::sync::Mutex::new(()),
        })
    }

    pub fn staged_path(&self) -> Option<PathBuf> {
        self.selection.lock().staged.clone()
    }

    async fn check_feed(&self) -> Result<Option<CheckResult>, UpdaterError> {
        let _operation = self.operation.lock().await;
        *self.selection.lock() = Selection::default();
        let bytes = self
            .fetch(&self.config.feed_url, self.config.max_feed_bytes)
            .await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let release: FeedRelease =
            serde_json::from_slice(&bytes).map_err(|e| UpdaterError::InvalidFeed(e.to_string()))?;
        let version = parse_version(&release.version)?;
        let current = parse_version(&self.config.current_version)?;
        // Electron explicitly allows downgrades but excludes prereleases on the latest channel.
        if version == current || !version.pre.is_empty() {
            return Ok(Some(CheckResult::default()));
        }
        let platform = release
            .platforms
            .get(&self.config.platform)
            .ok_or_else(|| UpdaterError::MissingPlatform(self.config.platform.clone()))?;
        validate_url(
            &Url::parse(&platform.url).map_err(|e| UpdaterError::InvalidFeed(e.to_string()))?,
        )?;
        if platform.signature.trim().is_empty() {
            return Err(UpdaterError::InvalidFeed("empty artifact signature".into()));
        }
        let result = CheckResult {
            is_update_available: true,
            version: Some(release.version.clone()),
        };
        self.selection.lock().release = Some(release);
        Ok(Some(result))
    }

    async fn download(&self) -> Result<(), UpdaterError> {
        let _operation = self.operation.lock().await;
        let release = {
            let mut selection = self.selection.lock();
            selection.staged = None;
            selection
                .release
                .clone()
                .ok_or(UpdaterError::NothingDownloaded)?
        };
        let platform = release
            .platforms
            .get(&self.config.platform)
            .ok_or_else(|| UpdaterError::MissingPlatform(self.config.platform.clone()))?;
        let url =
            Url::parse(&platform.url).map_err(|e| UpdaterError::InvalidFeed(e.to_string()))?;
        let bytes = self.fetch(&url, self.config.max_artifact_bytes).await?;
        self.verifier.verify(&bytes, &platform.signature)?;
        tokio::fs::create_dir_all(&self.config.staging_dir)
            .await
            .map_err(persistence)?;
        // A unique file avoids overwriting prior stages and never derives a path from feed text.
        let path = self
            .config
            .staging_dir
            .join(format!("{}.update", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(persistence)?;
        use tokio::io::AsyncWriteExt;
        if let Err(error) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(persistence(error));
        }
        if let Err(error) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(persistence(error));
        }
        self.selection.lock().staged = Some(path);
        Ok(())
    }

    async fn fetch(&self, url: &Url, limit: usize) -> Result<Vec<u8>, UpdaterError> {
        validate_url(url)?;
        let mut response = self.client.get(url.clone()).send().await.map_err(network)?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(UpdaterError::Network {
                message: format!("HTTP {}", response.status()),
                retryable: response.status().is_server_error()
                    || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS,
            });
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(UpdaterError::InvalidFeed(
                "response exceeds byte limit".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(UpdaterError::InvalidFeed(
                    "response exceeds byte limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(UpdaterError::InvalidFeed(
                "empty successful response".into(),
            ));
        }
        Ok(bytes)
    }
}

impl UpdaterBackend for FeedBackend {
    fn check_for_updates(&self) -> BoxFuture<'_, Result<Option<CheckResult>, UpdaterError>> {
        Box::pin(self.check_feed())
    }
    fn download_update(&self) -> BoxFuture<'_, Result<(), UpdaterError>> {
        Box::pin(self.download())
    }
    fn quit_and_install(&self) -> Result<(), UpdaterError> {
        let (path, release) = {
            let selection = self.selection.lock();
            (
                selection
                    .staged
                    .clone()
                    .ok_or(UpdaterError::NothingDownloaded)?,
                selection
                    .release
                    .clone()
                    .ok_or(UpdaterError::NothingDownloaded)?,
            )
        };
        // Reverify disk bytes before handing off. Installer adapters must preserve this
        // verified artifact identity until replacement, not reopen an untrusted path.
        use std::io::Read;
        let file = std::fs::File::open(&path).map_err(persistence)?;
        let mut bytes = Vec::new();
        file.take((self.config.max_artifact_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(persistence)?;
        if bytes.len() > self.config.max_artifact_bytes {
            return Err(UpdaterError::InvalidFeed(
                "staged artifact exceeds byte limit".into(),
            ));
        }
        let platform = release
            .platforms
            .get(&self.config.platform)
            .ok_or_else(|| UpdaterError::MissingPlatform(self.config.platform.clone()))?;
        self.verifier.verify(&bytes, &platform.signature)?;
        self.installer.install(&path, &release)
    }
}

fn parse_version(value: &str) -> Result<semver::Version, UpdaterError> {
    semver::Version::parse(value.strip_prefix('v').unwrap_or(value))
        .map_err(|e| UpdaterError::InvalidFeed(e.to_string()))
}

fn validate_url(url: &Url) -> Result<(), UpdaterError> {
    let loopback = url
        .host_str()
        .and_then(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    if (url.scheme() == "https" || (url.scheme() == "http" && loopback))
        && url.username().is_empty()
        && url.password().is_none()
    {
        return Ok(());
    }
    Err(UpdaterError::InvalidFeed(
        "URL must use HTTPS or numeric loopback HTTP without credentials".into(),
    ))
}

fn network(error: reqwest::Error) -> UpdaterError {
    UpdaterError::Network {
        retryable: error.is_timeout() || error.is_connect() || error.is_body(),
        message: error.to_string(),
    }
}

fn persistence(error: std::io::Error) -> UpdaterError {
    UpdaterError::Persistence(error.to_string())
}
