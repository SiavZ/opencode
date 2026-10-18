//! Purpose: external configuration sources for the legacy loader, behind narrow traits:
//! well-known/remote org config over HTTP (owned by Phase 03/06 transport), active account org
//! config, and macOS MDM managed preferences. Defaults are typed `Unsupported`/empty.
//! Scope: traits, std-backed managed-preferences reader, response validation.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! `-- config/src/fs.rs
//! External:
//! |-- serde_json::Value
//! `-- std::process::Command
//! Used by:
//! rust/crates/
//! `-- config/src/legacy/load.rs
//! Invariants: an HTML body or `content-type: *html*` at HTTP 200 is a `ConfigRemoteAuthError`
//! with the login origin; non-object remote payloads fail; MDM payload metadata keys are dropped.
//! Contract tests: config/tests/legacy_remote.rs

use crate::env::{Environment, Platform};
use crate::error::{ConfigError, RemoteError};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One HTTP response from a remote config endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

/// HTTP GET transport for remote config (Accept: application/json, transient-read retries are
/// the transport's job). Phase 03/06 provide the production adapter.
pub trait RemoteFetcher: Send + Sync {
    fn get(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<RemoteResponse, RemoteError>;
}

/// Default transport: every fetch fails with a typed `Unsupported` error.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedRemote;

impl RemoteFetcher for UnsupportedRemote {
    fn get(
        &self,
        url: &str,
        _headers: &BTreeMap<String, String>,
    ) -> Result<RemoteResponse, RemoteError> {
        Err(RemoteError::Unsupported(url.to_string()))
    }
}

/// `Auth.WellKnown` entries: `{ [url]: { type: "wellknown", key, token } }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WellKnownAuth {
    pub url: String,
    pub key: String,
    pub token: String,
}

/// Active console account with an org (`Account.active()` + `config` + `token`).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountOrg {
    pub url: String,
    pub config: Option<Value>,
    pub token: Option<String>,
}

/// Auth/account inputs. Phase 06 owns the stores; tests inject fixtures.
pub trait AccountSource: Send + Sync {
    fn wellknown(&self) -> Vec<WellKnownAuth> {
        Vec::new()
    }
    /// Errors here are swallowed by the loader, like `Effect.catch` on the org fetch.
    fn active_org(&self) -> Result<Option<AccountOrg>, ConfigError> {
        Ok(None)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoAccounts;

impl AccountSource for NoAccounts {}

/// macOS managed preferences: `(source, json_text)` with MDM metadata stripped.
pub trait ManagedPreferences: Send + Sync {
    fn read(&self, env: &Environment) -> Option<(String, String)>;
}

const PLIST_DOMAIN: &str = "ai.opencode.managed";
const PLIST_META: [&str; 6] = [
    "PayloadDisplayName",
    "PayloadIdentifier",
    "PayloadType",
    "PayloadUUID",
    "PayloadVersion",
    "_manualProfile",
];

/// `parseManagedPlist`: drop MDM metadata keys from `plutil -convert json` output.
pub fn parse_managed_plist(json: &str) -> Result<String, ConfigError> {
    let mut raw: Value = serde_json::from_str(json).map_err(|error| ConfigError::Json {
        path: "mobileconfig".into(),
        message: error.to_string(),
    })?;
    if let Some(map) = raw.as_object_mut() {
        map.retain(|key, _| !PLIST_META.contains(&key.as_str()));
    }
    Ok(raw.to_string())
}

/// `readManagedPreferences` using `/Library/Managed Preferences` and `plutil` on macOS.
#[derive(Debug, Default, Clone)]
pub struct SystemManagedPreferences {
    /// Override for tests; production uses `/Library/Managed Preferences`.
    pub root: Option<PathBuf>,
}

impl ManagedPreferences for SystemManagedPreferences {
    fn read(&self, env: &Environment) -> Option<(String, String)> {
        if env.platform != Platform::MacOs {
            return None;
        }
        let root = self
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from("/Library/Managed Preferences"));
        let file = format!("{PLIST_DOMAIN}.plist");
        let paths = [
            root.join(env.username_or_default()).join(&file),
            root.join(&file),
        ];
        paths
            .iter()
            .filter(|plist| plist.exists())
            .find_map(|plist| {
                let output = std::process::Command::new("plutil")
                    .args(["-convert", "json", "-o", "-"])
                    .arg(plist)
                    .output()
                    .ok()
                    .filter(|output| output.status.success())?;
                let text = parse_managed_plist(&String::from_utf8_lossy(&output.stdout)).ok()?;
                Some((format!("mobileconfig:{}", plist.display()), text))
            })
    }
}

/// `fetchRemoteJson`: status, HTML-login and JSON checks.
pub fn fetch_json(
    fetcher: &dyn RemoteFetcher,
    url: &str,
    headers: &BTreeMap<String, String>,
    login_origin: &str,
) -> Result<Value, ConfigError> {
    let response = fetcher.get(url, headers)?;
    if !(200..300).contains(&response.status) {
        let message = format!("StatusCode: non 2xx status code ({})", response.status);
        return Err(RemoteError::Fetch {
            url: url.into(),
            message,
        }
        .into());
    }
    let lower = response.content_type.to_lowercase();
    let head = response.body.trim_start().to_lowercase();
    if lower.contains("html") || head.starts_with("<!doctype") || head.starts_with("<html") {
        return Err(ConfigError::RemoteAuth {
            url: login_origin.into(),
            remote: url.into(),
        });
    }
    serde_json::from_str(&response.body).map_err(|error| {
        RemoteError::Decode {
            url: url.into(),
            message: error.to_string(),
        }
        .into()
    })
}
