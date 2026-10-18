//! Purpose: legacy provider auth store, port of packages/opencode/src/auth/index.ts:52-95
//! (`Auth.Service`: all/get/set/remove over `<data>/auth.json` or `OPENCODE_AUTH_CONTENT`).
//! Scope: async file I/O on the given data dir. The env override is passed in explicitly.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! |-- storage-sqlite/src/legacy_files.rs
//! `-- storage-sqlite/src/legacy_files/store.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/legacy_files.rs
//! `-- storage-sqlite/tests/credentials.rs
//! Invariants: when the env content is non-empty and parses as JSON it is returned unvalidated (TS
//! `JSON.parse` result). Otherwise the file is read, a missing or invalid file is `{}`, and entries that
//! fail the `Auth.Info` union are dropped, valid ones are re-encoded (excess keys stripped). `set`
//! strips trailing `/` from the key, deletes the raw key and `<norm>/`, writes `{...data, [norm]: info}`
//! (an existing key keeps its position). `remove` deletes the raw and normalized keys. Writes are
//! `JSON.stringify(data, null, 2)` followed by chmod 0600, they do not create the data dir, and fail
//! with "Failed to write auth data". Secrets never appear in `Debug` output.
//! Known divergence: a non-object env JSON (array, string, null) is treated as `{}`, where TS would
//! hand the raw value to callers.
//! Contract tests: storage-sqlite/tests/credentials.rs

use std::path::{Path, PathBuf};

use opencode_storage_api::{Result, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::js_pretty;

/// Environment variable carrying inline auth JSON.
pub const AUTH_CONTENT_ENV: &str = "OPENCODE_AUTH_CONTENT";
/// Placeholder key used by OAuth providers (`OAUTH_DUMMY_KEY`).
pub const OAUTH_DUMMY_KEY: &str = "opencode-oauth-dummy-key";

/// `Auth.Info`: `Oauth | Api | WellKnown`, discriminated by `type`. Field order equals the schema.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthInfo {
    Oauth {
        refresh: String,
        access: String,
        expires: u64,
        #[serde(rename = "accountId", default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        #[serde(
            rename = "enterpriseUrl",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        enterprise_url: Option<String>,
    },
    Api {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Value>>,
    },
    #[serde(rename = "wellknown")]
    WellKnown { key: String, token: String },
}

impl std::fmt::Debug for AuthInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthInfo::Oauth { expires, .. } => f
                .debug_struct("Oauth")
                .field("expires", expires)
                .finish_non_exhaustive(),
            AuthInfo::Api { .. } => f.debug_struct("Api").finish_non_exhaustive(),
            AuthInfo::WellKnown { .. } => f.debug_struct("WellKnown").finish_non_exhaustive(),
        }
    }
}

impl AuthInfo {
    /// `Schema.decodeUnknownOption(Info)`: excess keys are ignored, `expires` must be a non-negative
    /// integer (integral floats accepted), `metadata` values must be strings.
    pub fn decode(value: &Value) -> Option<AuthInfo> {
        let object = value.as_object()?;
        let known: &[&str] = match object.get("type")?.as_str()? {
            "oauth" => &[
                "type",
                "refresh",
                "access",
                "expires",
                "accountId",
                "enterpriseUrl",
            ],
            "api" => &["type", "key", "metadata"],
            "wellknown" => &["type", "key", "token"],
            _ => return None,
        };
        let mut trimmed: Map<String, Value> = object
            .iter()
            .filter(|(k, _)| known.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(expires) = trimmed.get_mut("expires") {
            *expires = super::js_normalize(expires);
        }
        if let Some(Value::Object(metadata)) = trimmed.get("metadata")
            && metadata.values().any(|v| !v.is_string())
        {
            return None;
        }
        serde_json::from_value(Value::Object(trimmed)).ok()
    }

    fn encode(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Legacy `auth.json` store.
#[derive(Clone)]
pub struct LegacyAuth {
    file: PathBuf,
    env_content: Option<String>,
}

impl std::fmt::Debug for LegacyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyAuth")
            .field("file", &self.file)
            .field(
                "env_content",
                &self.env_content.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl LegacyAuth {
    /// Store at `<data_dir>/auth.json` with an explicit `OPENCODE_AUTH_CONTENT` value.
    pub fn new(data_dir: impl AsRef<Path>, env_content: Option<String>) -> Self {
        Self {
            file: data_dir.as_ref().join("auth.json"),
            env_content,
        }
    }

    /// Store reading `OPENCODE_AUTH_CONTENT` from the process environment.
    pub fn from_env(data_dir: impl AsRef<Path>) -> Self {
        Self::new(data_dir, std::env::var(AUTH_CONTENT_ENV).ok())
    }

    /// `<data_dir>/auth.json`.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `Auth.all`.
    pub async fn all(&self) -> Result<Map<String, Value>> {
        if let Some(content) = self.env_content.as_deref().filter(|c| !c.is_empty())
            && let Ok(parsed) = serde_json::from_str::<Value>(content)
        {
            return Ok(match parsed {
                Value::Object(map) => map,
                _ => Map::new(),
            });
        }
        let data = match tokio::fs::read_to_string(&self.file).await {
            Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Object(Map::new())),
            Err(_) => Value::Object(Map::new()),
        };
        let Value::Object(data) = data else {
            return Ok(Map::new());
        };
        Ok(data
            .iter()
            .filter_map(|(k, v)| AuthInfo::decode(v).map(|info| (k.clone(), info.encode())))
            .collect())
    }

    /// `Auth.get`, decoded. Unvalidated env entries that do not match the union read as `None`.
    pub async fn get(&self, provider_id: &str) -> Result<Option<AuthInfo>> {
        Ok(self
            .all()
            .await?
            .get(provider_id)
            .and_then(AuthInfo::decode))
    }

    /// `Auth.set`.
    pub async fn set(&self, key: &str, info: &AuthInfo) -> Result<()> {
        let norm = key.trim_end_matches('/');
        let mut data = self.all().await?;
        if norm != key {
            data.shift_remove(key);
        }
        data.shift_remove(&format!("{norm}/"));
        match data.get_mut(norm) {
            Some(slot) => *slot = info.encode(),
            None => {
                data.insert(norm.to_owned(), info.encode());
            }
        }
        self.write(&data).await
    }

    /// `Auth.remove`.
    pub async fn remove(&self, key: &str) -> Result<()> {
        let norm = key.trim_end_matches('/');
        let mut data = self.all().await?;
        data.shift_remove(key);
        data.shift_remove(norm);
        self.write(&data).await
    }

    /// `fsys.writeJson(file, data, 0o600)`.
    async fn write(&self, data: &Map<String, Value>) -> Result<()> {
        let fail = |error: std::io::Error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                StorageError::PermissionDenied(format!("Failed to write auth data: {error}"))
            }
            _ => StorageError::Io(format!("Failed to write auth data: {error}")),
        };
        tokio::fs::write(&self.file, js_pretty(&Value::Object(data.clone())))
            .await
            .map_err(fail)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&self.file, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(fail)?;
        }
        Ok(())
    }
}
