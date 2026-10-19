//! Purpose: proxy environment handling (loopback NO_PROXY upsert, env proxy selection) and the
//! shared HTTP client used for health checks, trusting the OS certificate store.
//! Scope: pure env transforms plus reqwest client construction. Never mutates the process env.
//! Uses:
//! `-- none
//! External:
//! |-- reqwest::Client
//! |-- reqwest::Client::builder
//! |-- reqwest::Error
//! |-- reqwest::NoProxy::from_string
//! |-- reqwest::Proxy::http
//! |-- reqwest::Proxy::https
//! |-- str::to_string
//! `-- str::trim
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections/background_cli.rs
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! |-- desktop-host/src/platform/mod.rs
//! |-- desktop-host/src/platform/shell_env.rs
//! |-- desktop-host/src/platform/wsl/sidecar.rs
//! |-- desktop-host/tests/connections.rs
//! |-- desktop-host/tests/connections_live.rs
//! `-- desktop-host/tests/platform.rs
//! Invariants: loopback hosts are appended once (case-insensitive) to both NO_PROXY and no_proxy,
//! existing entries keep their order; lowercase proxy vars win over uppercase (undici EnvHttpProxyAgent).
//! Contract tests: desktop-host/tests/platform.rs

use std::collections::BTreeMap;

pub type EnvMap = BTreeMap<String, String>;

pub const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid proxy url {url}: {message}")]
    InvalidProxy { url: String, message: String },
    #[error("failed to build http client: {0}")]
    Client(String),
}

/// Port of `ensureLoopbackNoProxy` (`packages/desktop/src/main/index.ts:95-113`, sidecar.ts:83-101).
pub fn upsert_loopback(value: Option<&str>) -> String {
    let mut items: Vec<String> = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    for host in LOOPBACK_HOSTS {
        if items.iter().any(|item| item.to_lowercase() == host) {
            continue;
        }
        items.push(host.to_string());
    }
    items.join(",")
}

pub fn ensure_loopback_no_proxy(env: &mut EnvMap) {
    for key in ["NO_PROXY", "no_proxy"] {
        let next = upsert_loopback(env.get(key).map(String::as_str));
        env.insert(key.to_string(), next);
    }
}

/// Proxy settings selected from an env map, mirroring Node 24 `http.setGlobalProxyFromEnv`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvProxy {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
}

impl EnvProxy {
    pub fn from_env(env: &EnvMap) -> Self {
        // JS `env.lower || env.UPPER`: an empty lowercase value falls through to the uppercase one.
        let pick = |lower: &str, upper: &str| {
            [lower, upper]
                .iter()
                .filter_map(|key| env.get(*key))
                .find(|value| !value.is_empty())
                .cloned()
        };
        Self {
            http: pick("http_proxy", "HTTP_PROXY"),
            https: pick("https_proxy", "HTTPS_PROXY"),
            no_proxy: pick("no_proxy", "NO_PROXY"),
        }
    }
}

/// Build the host HTTP client: explicit env proxies (system proxy auto-detection disabled so the
/// env is the single source of truth), and rustls with the platform verifier, which trusts the OS
/// certificate store (the Electron host merged Node default + system CAs, see notes/connections.md).
pub fn http_client(env: &EnvMap) -> Result<reqwest::Client, ProxyError> {
    let proxy = EnvProxy::from_env(env);
    let no_proxy = || {
        proxy
            .no_proxy
            .as_deref()
            .and_then(reqwest::NoProxy::from_string)
    };
    let mut builder = reqwest::Client::builder().no_proxy();
    if let Some(url) = &proxy.http {
        builder = builder.proxy(
            reqwest::Proxy::http(url)
                .map_err(|error| invalid(url, error))?
                .no_proxy(no_proxy()),
        );
    }
    if let Some(url) = &proxy.https {
        builder = builder.proxy(
            reqwest::Proxy::https(url)
                .map_err(|error| invalid(url, error))?
                .no_proxy(no_proxy()),
        );
    }
    builder
        .build()
        .map_err(|error| ProxyError::Client(error.to_string()))
}

/// Snapshot of the current process env as an [`EnvMap`] (non-UTF-8 entries are skipped).
pub fn process_env() -> EnvMap {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

fn invalid(url: &str, error: reqwest::Error) -> ProxyError {
    ProxyError::InvalidProxy {
        url: url.to_string(),
        message: error.to_string(),
    }
}
