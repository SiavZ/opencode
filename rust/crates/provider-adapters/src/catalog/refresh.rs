//! Purpose: fetch and atomically replace the explicit models.dev cache.
//! Scope: caller-owned HTTP client, cache path and lifecycle, with no ambient environment.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/catalog/mod.rs
//! External:
//! |-- reqwest::Client
//! `-- tokio::sync::Mutex
//! Used by:
//! `-- none, entrypoint
//! Invariants: failed fetches never replace a valid cache; concurrent calls are serialized.
//! Contract tests: provider-adapters/tests/catalog_refresh.rs

use super::{CatalogError, CatalogStore};
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

pub fn cache_filename(source: &str) -> String {
    if source == "https://models.opencode.ai" {
        return "models.json".into();
    }
    let hash: String = Sha1::digest(source.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("models-{hash}.json")
}

pub struct ModelsService {
    client: reqwest::Client,
    source: String,
    cache: PathBuf,
    user_agent: String,
    state: Mutex<Option<CatalogStore>>,
    disabled: bool,
    snapshot: Option<CatalogStore>,
    read_path: Option<PathBuf>,
    refreshed: tokio::sync::broadcast::Sender<()>,
}

impl ModelsService {
    pub fn new(
        client: reqwest::Client,
        source: String,
        cache: PathBuf,
        user_agent: String,
    ) -> Self {
        Self {
            client,
            source,
            cache,
            user_agent,
            state: Mutex::new(None),
            disabled: false,
            snapshot: None,
            read_path: None,
            refreshed: tokio::sync::broadcast::channel(16).0,
        }
    }

    /// Explicit host policy. Disabling downloads never disables an explicit refresh request.
    pub fn with_policy(mut self, disabled: bool, snapshot: Option<CatalogStore>) -> Self {
        self.disabled = disabled;
        self.snapshot = snapshot;
        self
    }

    /// A models-path override is read-only: refresh still updates the regular cache.
    pub fn with_read_path(mut self, path: Option<PathBuf>) -> Self {
        self.read_path = path;
        self
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.refreshed.subscribe()
    }

    pub async fn get(&self) -> Result<CatalogStore, CatalogError> {
        let mut state = self.state.lock().await;
        if let Some(store) = state.as_ref() {
            return Ok(store.clone());
        }
        let mut store = CatalogStore::new();
        let loaded = self.read_path.as_ref().unwrap_or(&self.cache).exists()
            && store
                .load_cache(self.read_path.as_ref().unwrap_or(&self.cache))
                .is_ok();
        if !loaded && let Some(snapshot) = &self.snapshot {
            *state = Some(snapshot.clone());
            return Ok(snapshot.clone());
        }
        if !loaded && !self.disabled {
            let _lock = self.lock().await?;
            if !self.cache.exists() || store.load_cache(&self.cache).is_err() {
                self.fetch().await?;
            }
            store.load_cache(&self.cache)?;
        }
        *state = Some(store.clone());
        Ok(store)
    }

    pub async fn environment_names(
        &self,
    ) -> Result<indexmap::IndexMap<opencode_schema::ids::IntegrationID, Vec<String>>, CatalogError>
    {
        Ok(self.get().await?.environment_names())
    }

    /// Returns whether inventory was replaced. Errors are observable to explicit callers.
    pub async fn refresh(&self, force: bool) -> Result<bool, CatalogError> {
        let mut state = self.state.lock().await;
        if !force && self.fresh() {
            return Ok(false);
        }
        let _lock = self.lock().await?;
        if !force && self.fresh() {
            return Ok(false);
        }
        self.fetch().await?;
        *state = None;
        let _ = self.refreshed.send(());
        Ok(true)
    }

    async fn lock(&self) -> Result<std::fs::File, CatalogError> {
        let path = self.cache.with_extension("json.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    fn fresh(&self) -> bool {
        std::fs::metadata(&self.cache)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < Duration::from_secs(300))
    }

    async fn fetch(&self) -> Result<(), CatalogError> {
        tokio::time::timeout(Duration::from_secs(10), self.fetch_inner())
            .await
            .map_err(|_| CatalogError::Fetch("catalog request timed out".into()))?
    }

    async fn fetch_inner(&self) -> Result<(), CatalogError> {
        let url = format!("{}/api.json", self.source.trim_end_matches('/'));
        let mut attempt = 0;
        let bytes = loop {
            let result = self
                .client
                .get(&url)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            match result {
                Ok(response) if response.status().is_success() => {
                    break response
                        .bytes()
                        .await
                        .map_err(|_| CatalogError::Fetch("cannot read catalog response".into()))?;
                }
                Ok(response)
                    if (response.status().is_server_error()
                        || response.status().as_u16() == 429)
                        && attempt < 2 => {}
                Ok(response) => {
                    return Err(CatalogError::Fetch(format!(
                        "catalog HTTP {}",
                        response.status().as_u16()
                    )));
                }
                Err(_) if attempt < 2 => {}
                Err(_) => return Err(CatalogError::Fetch("catalog request failed".into())),
            }
            tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
            attempt += 1;
        };
        // Validate before persisting so a broken remote document cannot poison the last good cache.
        let mut store = CatalogStore::new();
        super::cache::decode(&mut store, &bytes)?;
        if let Some(parent) = self.cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.cache.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        if let Err(error) = std::fs::write(&temporary, &bytes)
            .and_then(|()| std::fs::rename(&temporary, &self.cache))
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }
}
