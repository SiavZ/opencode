//! Purpose: forward migrated Console inference requests to their destination service.
//! Scope: migration lookup, custom BYOK routing and streamed HTTP transport.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-domain/src/console/inference.rs
//! External:
//! |-- http::{HeaderMap, Method}
//! |-- mysql_async::prelude::Queryable
//! `-- reqwest::{Client, Url}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen.rs
//! Invariants: migrated destination owns revocation, limits and usage; no local double billing.
//! Contract tests: hosted-adapters/tests/console_gateway.rs

pub mod body;
pub mod catalog;
pub mod native;
pub mod usage;

use http::{HeaderMap, HeaderValue, Method};
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::inference;
use reqwest::{Client, Url};

use super::db::{ConsoleDb, DbError};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Mysql(#[from] mysql_async::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("Legacy BYOK model mapping is unavailable")]
    MissingModel,
    #[error("Invalid inference destination configuration")]
    Configuration,
}

#[derive(Clone)]
pub struct MigrationGateway {
    client: Client,
    destination: Url,
    access_client_id: HeaderValue,
}

impl std::fmt::Debug for MigrationGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationGateway").finish_non_exhaustive()
    }
}

pub struct MigrationTarget {
    pub url: Url,
    pub headers: HeaderMap,
    pub model: Option<String>,
}

impl MigrationGateway {
    pub fn new(destination: &str, access_client_id: &str) -> Result<Self, GatewayError> {
        let destination = Url::parse(destination).map_err(|_| GatewayError::Configuration)?;
        if !matches!(destination.scheme(), "http" | "https")
            || destination.host_str().is_none()
            || !destination.username().is_empty()
            || destination.password().is_some()
        {
            return Err(GatewayError::Configuration);
        }
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            destination,
            access_client_id: HeaderValue::from_str(access_client_id)
                .map_err(|_| GatewayError::Configuration)?,
        })
    }

    pub async fn target(
        &self,
        db: &ConsoleDb,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        mapping: Option<(&str, &str)>,
    ) -> Result<Option<MigrationTarget>, GatewayError> {
        let Some(mut target) = inference::migrated_path(method.as_str(), path) else {
            return Ok(None);
        };
        let name = inference::api_key_header(path);
        let key = headers.get(name).and_then(|value| value.to_str().ok());
        let key = if name == "authorization" {
            key.and_then(|value| value.split(' ').nth(1))
        } else {
            key
        };
        let Some(key) = key.filter(|key| !key.is_empty() && *key != "public") else {
            return Ok(None);
        };
        let mut model = None;
        if !key.starts_with("oc_sk_") {
            let mut conn = db.conn().await?;
            // Cutover routing deliberately does not check key/workspace deletion: the destination owns revocation.
            let row: Option<(String, Option<String>)> = conn.exec_first(
                "SELECT w.id, p.provider FROM `key` k INNER JOIN workspace w ON w.id = k.workspace_id LEFT JOIN provider p ON p.workspace_id = k.workspace_id AND p.provider = ? AND p.time_deleted IS NULL AND length(p.credentials) > 0 WHERE k.`key` = ? AND w.migrated_at IS NOT NULL LIMIT 1",
                (mapping.map(|m| m.0), key),
            ).await?;
            let Some((workspace, provider)) = row else {
                return Ok(None);
            };
            if let Some(provider) = provider {
                let native_model = mapping
                    .map(|m| m.1)
                    .filter(|value| !value.is_empty())
                    .ok_or(GatewayError::MissingModel)?;
                let encoded_model: String =
                    url::form_urlencoded::byte_serialize(native_model.as_bytes())
                        .collect::<String>()
                        .replace('+', "%20");
                target =
                    inference::custom_connection_path(path, &workspace, &provider, &encoded_model)
                        .ok_or(GatewayError::MissingModel)?;
                model = Some(native_model.into());
            }
        }
        let mut url = self.destination.clone();
        url.set_path(&format!("{}{target}", url.path().trim_end_matches('/')));
        url.set_query(query);
        url.set_fragment(None);
        let mut forwarded = headers.clone();
        for name in [
            "x-zen",
            "x-zen-model",
            "x-zen-ip",
            "cf-access-client-id",
            "cf-access-client-secret",
            "host",
            "content-length",
        ] {
            forwarded.remove(name);
        }
        forwarded.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| GatewayError::Configuration)?,
        );
        forwarded.insert("cf-access-client-id", self.access_client_id.clone());
        if let Some(ip) = headers.get("cf-connecting-ip") {
            forwarded.insert("x-zen-ip", ip.clone());
        }
        if let Some(id) = headers
            .get("x-opencode-request-id")
            .or_else(|| headers.get("x-opencode-request"))
        {
            forwarded.insert("x-opencode-request-id", id.clone());
        }
        Ok(Some(MigrationTarget {
            url,
            headers: forwarded,
            model,
        }))
    }

    pub async fn forward(
        &self,
        method: Method,
        target: MigrationTarget,
        body: reqwest::Body,
    ) -> Result<reqwest::Response, GatewayError> {
        Ok(self
            .client
            .request(method, target.url)
            .headers(target.headers)
            .body(body)
            .send()
            .await?)
    }
}
