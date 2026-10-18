//! Purpose: MCP OAuth credential store (`mcp-auth.json`), port of packages/opencode/src/mcp/auth.ts.
//! Scope: one JSON file under an injected data directory, written with mode 0600 and guarded by
//! an exclusive file lock (`mcp-auth.json.lock`) plus an in-process mutex.
//! Uses:
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/oauth_provider.rs
//! `-- integration-adapters/src/mcp/service.rs
//! Invariants: never touches the real user data dir unless the caller passes it; unreadable or
//! invalid files read as `{}`; `get_for_url` only returns entries bound to the exact URL;
//! numbers are written JS-style (integral floats without `.0`).
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub expires_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub client_id_issued_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub client_secret_expires_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
}

pub type AuthData = BTreeMap<String, Entry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Tokens,
    ClientInfo,
    CodeVerifier,
    OAuthState,
}

#[derive(Clone)]
pub struct McpAuthStore {
    path: PathBuf,
    guard: Arc<Mutex<()>>,
}

impl McpAuthStore {
    /// `data_dir` is the opencode data directory (`Global.Path.data`), injected by the caller.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: data_dir.into().join("mcp-auth.json"),
            guard: Arc::default(),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub async fn all(&self) -> AuthData {
        let _g = self.guard.lock().await;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || locked(&path, |p| Ok(read(p))))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    pub async fn get(&self, name: &str) -> Option<Entry> {
        self.all().await.remove(name)
    }

    pub async fn get_for_url(&self, name: &str, server_url: &str) -> Option<Entry> {
        self.get(name)
            .await
            .filter(|e| e.server_url.as_deref() == Some(server_url))
    }

    /// Read-modify-write under the lock. `update` returning `None` skips the write.
    pub async fn mutate<F>(&self, update: F) -> std::io::Result<()>
    where
        F: FnOnce(AuthData) -> Option<AuthData> + Send + 'static,
    {
        let _g = self.guard.lock().await;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            locked(&path, |p| match update(read(p)) {
                Some(next) => write(p, &next),
                None => Ok(()),
            })
        })
        .await
        .map_err(std::io::Error::other)?
    }

    pub async fn set(
        &self,
        name: &str,
        entry: Entry,
        server_url: Option<&str>,
    ) -> std::io::Result<()> {
        let (name, url) = (name.to_string(), server_url.map(str::to_string));
        self.mutate(move |mut data| {
            let mut entry = entry;
            if let Some(url) = url {
                entry.server_url = Some(url);
            }
            data.insert(name, entry);
            Some(data)
        })
        .await
    }

    pub async fn remove(&self, name: &str) -> std::io::Result<()> {
        let name = name.to_string();
        self.mutate(move |mut data| {
            data.remove(&name);
            Some(data)
        })
        .await
    }

    pub async fn update_tokens(
        &self,
        name: &str,
        tokens: Tokens,
        url: Option<&str>,
    ) -> std::io::Result<()> {
        self.update(name, url, move |e| e.tokens = Some(tokens))
            .await
    }

    pub async fn update_client_info(
        &self,
        name: &str,
        info: ClientInfo,
        url: Option<&str>,
    ) -> std::io::Result<()> {
        self.update(name, url, move |e| e.client_info = Some(info))
            .await
    }

    pub async fn update_code_verifier(&self, name: &str, verifier: &str) -> std::io::Result<()> {
        let v = verifier.to_string();
        self.update(name, None, move |e| e.code_verifier = Some(v))
            .await
    }

    pub async fn update_oauth_state(&self, name: &str, state: &str) -> std::io::Result<()> {
        let v = state.to_string();
        self.update(name, None, move |e| e.oauth_state = Some(v))
            .await
    }

    pub async fn get_oauth_state(&self, name: &str) -> Option<String> {
        self.get(name).await.and_then(|e| e.oauth_state)
    }

    pub async fn clear(&self, name: &str, field: Field) -> std::io::Result<()> {
        let name = name.to_string();
        self.mutate(move |mut data| {
            let entry = data.get_mut(&name)?;
            match field {
                Field::Tokens => entry.tokens = None,
                Field::ClientInfo => entry.client_info = None,
                Field::CodeVerifier => entry.code_verifier = None,
                Field::OAuthState => entry.oauth_state = None,
            }
            Some(data)
        })
        .await
    }

    async fn update<F>(&self, name: &str, url: Option<&str>, apply: F) -> std::io::Result<()>
    where
        F: FnOnce(&mut Entry) + Send + 'static,
    {
        let (name, url) = (name.to_string(), url.map(str::to_string));
        self.mutate(move |mut data| {
            let entry = data.entry(name).or_default();
            apply(entry);
            if let Some(url) = url {
                entry.server_url = Some(url);
            }
            Some(data)
        })
        .await
    }
}

fn locked<T>(
    path: &std::path::Path,
    f: impl FnOnce(&std::path::Path) -> std::io::Result<T>,
) -> std::io::Result<T> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"))?;
    lock.lock()?;
    let out = f(path);
    let _ = lock.unlock();
    out
}

fn read(path: &std::path::Path) -> AuthData {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(path: &std::path::Path, data: &AuthData) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(data).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    // The baseline chmods after writing so pre-existing files are tightened too.
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

/// JS numbers: integral values serialize without a fractional part, like `JSON.stringify`.
mod js_number {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => s.serialize_i64(*n as i64),
            Some(n) => s.serialize_f64(*n),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Option::<f64>::deserialize(d)
    }
}
