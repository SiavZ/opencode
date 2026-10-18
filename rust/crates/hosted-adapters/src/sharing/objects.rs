//! Purpose: enterprise share object store: the `Storage.Adapter` port (storage.ts:5-10), the
//! S3/R2 HTTP adapter over SigV4-signed reqwest calls (storage.ts:12-84) and the JSON key layer
//! (storage.ts:93-128).
//! Scope: async I/O. One HTTP request per call, no retries (the baseline has none).
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/sharing/sigv4.rs
//! External:
//! |-- async_trait::async_trait
//! |-- reqwest::Client
//! |-- serde_json
//! `-- time::OffsetDateTime
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/mod.rs
//! |-- hosted-adapters/src/sharing/service.rs
//! `-- hosted-adapters/tests/sharing_baseline.rs
//! Invariants: list reads ONE ListObjectsV2 page (no continuation, faithful defect), `after` maps to
//! `start-after=<prefix><after>.json`, `before` filters keys `< <prefix><before>.json` client side,
//! keys are extracted with the `<Key>([^<]+)</Key>` rule without XML unescaping (faithful), 404 on
//! read is `None`, every other non-2xx is an error carrying the status.
//! Contract tests: hosted-adapters/tests/sharing_baseline.rs

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use time::OffsetDateTime;
use time::macros::format_description;
use url::Url;

use crate::sharing::sigv4::{Credentials, region_for, sign};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("Failed to {op} {path}: {status}")]
    Status {
        op: &'static str,
        path: String,
        status: u16,
    },
    #[error("storage transport error: {0}")]
    Transport(String),
    #[error("invalid stored JSON at {path}: {message}")]
    Decode { path: String, message: String },
}

#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub prefix: String,
    pub limit: Option<u32>,
    pub after: Option<String>,
    pub before: Option<String>,
}

/// `Storage.Adapter`. Paths are full object keys including `.json`.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn read(&self, path: &str) -> Result<Option<String>, StoreError>;
    async fn write(&self, path: &str, value: String) -> Result<(), StoreError>;
    async fn remove(&self, path: &str) -> Result<(), StoreError>;
    async fn list(&self, options: ListOptions) -> Result<Vec<String>, StoreError>;
}

/// S3-compatible adapter (`createAdapter`).
#[derive(Debug, Clone)]
pub struct S3Store {
    client: reqwest::Client,
    creds: Credentials,
    base: String,
    region: String,
}

impl S3Store {
    pub fn new(
        client: reqwest::Client,
        creds: Credentials,
        endpoint: &str,
        bucket: &str,
        region: &str,
    ) -> Self {
        Self {
            client,
            creds,
            base: format!("{endpoint}/{bucket}"),
            region: region.into(),
        }
    }

    /// `s3()` (storage.ts:66-75).
    pub fn aws(
        client: reqwest::Client,
        creds: Credentials,
        bucket: &str,
        region: Option<&str>,
    ) -> Self {
        let region = region.unwrap_or("us-east-1");
        Self::new(
            client,
            creds,
            &format!("https://s3.{region}.amazonaws.com"),
            bucket,
            region,
        )
    }

    /// `r2()` (storage.ts:77-84).
    pub fn r2(client: reqwest::Client, creds: Credentials, account_id: &str, bucket: &str) -> Self {
        Self::new(
            client,
            creds,
            &format!("https://{account_id}.r2.cloudflarestorage.com"),
            bucket,
            "auto",
        )
    }

    /// Adapter selection from `OPENCODE_STORAGE_*` settings (storage.ts:66-91). `get` reads one
    /// setting, missing values become empty strings like the baseline's non-null assertions.
    pub fn from_settings(get: impl Fn(&str) -> Option<String>) -> Result<Self, StoreError> {
        let value = |name: &str| get(name).unwrap_or_default();
        let creds = Credentials {
            access_key_id: value("OPENCODE_STORAGE_ACCESS_KEY_ID"),
            secret_access_key: value("OPENCODE_STORAGE_SECRET_ACCESS_KEY"),
        };
        let client = reqwest::Client::new();
        let bucket = value("OPENCODE_STORAGE_BUCKET");
        match get("OPENCODE_STORAGE_ADAPTER").as_deref() {
            Some("r2") => Ok(Self::r2(
                client,
                creds,
                &value("OPENCODE_STORAGE_ACCOUNT_ID"),
                &bucket,
            )),
            Some("s3") => {
                let region = get("OPENCODE_STORAGE_REGION").filter(|r| !r.is_empty());
                Ok(Self::aws(client, creds, &bucket, region.as_deref()))
            }
            _ => Err(StoreError::Transport(
                "No storage adapter configured".into(),
            )),
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<String>,
    ) -> Result<reqwest::Response, StoreError> {
        let parsed = Url::parse(url).map_err(|e| StoreError::Transport(e.to_string()))?;
        let headers = match body {
            Some(_) => vec![("content-type".to_string(), "application/json".to_string())],
            None => Vec::new(),
        };
        let datetime = OffsetDateTime::now_utc()
            .format(format_description!(
                "[year][month][day]T[hour][minute][second]Z"
            ))
            .map_err(|e| StoreError::Transport(e.to_string()))?;
        let region = region_for(&parsed, &self.region);
        let signed = sign(
            &self.creds,
            method.as_str(),
            &parsed,
            &headers,
            &region,
            &datetime,
        );
        let mut request = self.client.request(method, parsed);
        for (k, v) in signed {
            request = request.header(k, v);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        request
            .send()
            .await
            .map_err(|e| StoreError::Transport(e.to_string()))
    }
}

/// Encode an object key as a URL path the way `fetch` normalizes `${base}/${path}`.
fn object_url(base: &str, path: &str) -> String {
    let encoded: String = path
        .split('/')
        .map(|segment| percent_encoding::utf8_percent_encode(segment, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("{base}/{encoded}")
}

/// WHATWG URL path percent-encode set (space, `"`, `#`, `<`, `>`, `?`, backtick, `{`, `}`, controls).
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

#[async_trait]
impl ObjectStore for S3Store {
    async fn read(&self, path: &str) -> Result<Option<String>, StoreError> {
        let response = self
            .send(reqwest::Method::GET, &object_url(&self.base, path), None)
            .await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(StoreError::Status {
                op: "read",
                path: path.into(),
                status: response.status().as_u16(),
            });
        }
        let text = response
            .text()
            .await
            .map_err(|e| StoreError::Transport(e.to_string()))?;
        Ok(Some(text))
    }

    async fn write(&self, path: &str, value: String) -> Result<(), StoreError> {
        let response = self
            .send(
                reqwest::Method::PUT,
                &object_url(&self.base, path),
                Some(value),
            )
            .await?;
        if !response.status().is_success() {
            return Err(StoreError::Status {
                op: "write",
                path: path.into(),
                status: response.status().as_u16(),
            });
        }
        Ok(())
    }

    async fn remove(&self, path: &str) -> Result<(), StoreError> {
        let response = self
            .send(reqwest::Method::DELETE, &object_url(&self.base, path), None)
            .await?;
        if !response.status().is_success() {
            return Err(StoreError::Status {
                op: "remove",
                path: path.into(),
                status: response.status().as_u16(),
            });
        }
        Ok(())
    }

    async fn list(&self, options: ListOptions) -> Result<Vec<String>, StoreError> {
        let url = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("list-type", "2")
                .append_pair("prefix", &options.prefix);
            if let Some(limit) = options.limit.filter(|l| *l > 0) {
                query.append_pair("max-keys", &limit.to_string());
            }
            if let Some(after) = options.after.as_deref().filter(|a| !a.is_empty()) {
                query.append_pair("start-after", &format!("{}{after}.json", options.prefix));
            }
            format!("{}?{}", self.base, query.finish())
        };
        let response = self.send(reqwest::Method::GET, &url, None).await?;
        if !response.status().is_success() {
            return Err(StoreError::Status {
                op: "list",
                path: options.prefix,
                status: response.status().as_u16(),
            });
        }
        let xml = response
            .text()
            .await
            .map_err(|e| StoreError::Transport(e.to_string()))?;
        let keys = extract_keys(&xml);
        Ok(match options.before.as_deref().filter(|b| !b.is_empty()) {
            Some(before) => {
                let bound = format!("{}{before}.json", options.prefix);
                keys.into_iter()
                    .filter(|k| k.as_str() < bound.as_str())
                    .collect()
            }
            None => keys,
        })
    }
}

/// `/<Key>([^<]+)<\/Key>/g` without XML entity decoding.
pub fn extract_keys(xml: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Key>") {
        let after = &rest[start + 5..];
        let end = after.find('<').unwrap_or(after.len());
        if end > 0 && after[end..].starts_with("</Key>") {
            keys.push(after[..end].to_string());
        }
        rest = &after[end..];
    }
    keys
}

/// `Storage.read/write/remove/list/update` over segment keys.
#[derive(Clone, Copy)]
pub struct JsonStore<'a> {
    pub store: &'a dyn ObjectStore,
}

pub fn resolve(key: &[String]) -> String {
    format!("{}.json", key.join("/"))
}

impl JsonStore<'_> {
    pub async fn read<T: DeserializeOwned>(&self, key: &[String]) -> Result<Option<T>, StoreError> {
        let path = resolve(key);
        match self.store.read(&path).await? {
            // `if (!result) return undefined` (storage.ts:99): an empty body reads as missing.
            None => Ok(None),
            Some(text) if text.is_empty() => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| StoreError::Decode {
                    path,
                    message: e.to_string(),
                }),
        }
    }

    pub async fn write<T: Serialize + ?Sized>(
        &self,
        key: &[String],
        value: &T,
    ) -> Result<(), StoreError> {
        let text =
            serde_json::to_string(value).map_err(|e| StoreError::Transport(e.to_string()))?;
        self.store.write(&resolve(key), text).await
    }

    pub async fn remove(&self, key: &[String]) -> Result<(), StoreError> {
        self.store.remove(&resolve(key)).await
    }

    /// `Storage.list`: prefix segments joined with a trailing `/`, `.json` stripped, split on `/`.
    pub async fn list(
        &self,
        prefix: &[String],
        limit: Option<u32>,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<Vec<Vec<String>>, StoreError> {
        let joined = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.join("/"))
        };
        let keys = self
            .store
            .list(ListOptions {
                prefix: joined,
                limit,
                after,
                before,
            })
            .await?;
        Ok(keys
            .into_iter()
            .map(|k| {
                k.strip_suffix(".json")
                    .unwrap_or(&k)
                    .split('/')
                    .map(str::to_string)
                    .collect()
            })
            .collect())
    }
}
