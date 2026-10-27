//! Purpose: OpenAuth-compatible native issuer with verified providers and durable grant lifecycle.
//! Scope: configured hosted issuer; no unauthenticated identity creation API.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/issuer.rs
//! |-- hosted-adapters/src/console/auth/issuer_runtime/provider.rs
//! |-- hosted-adapters/src/console/auth/issuer_runtime/store.rs
//! |-- hosted-adapters/src/console/auth/issuer_runtime/token.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- jsonwebtoken::EncodingKey
//! |-- reqwest::Client
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/issuer.rs
//! Invariants: identities come only from verified provider responses; codes are consumed once.
//! Contract tests: issuer_runtime/tests.rs

mod provider;
mod store;
#[cfg(test)]
mod tests;
mod token;

use std::{collections::HashMap, sync::Arc};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use opencode_hosted_domain::console::identity::{
    IssuerOutcome, ProviderIdentity, is_allowed_authorization_redirect,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use provider::{GithubConfig, GoogleConfig};
pub use store::{IssuerStore, SqliteStore};

pub const ACCESS_TTL: i64 = 60 * 60 * 24 * 30;
pub const REFRESH_TTL: i64 = 60 * 60 * 24 * 365;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{code}: {description}")]
    OAuth {
        code: &'static str,
        description: &'static str,
        status: u16,
    },
    #[error("invalid issuer configuration: {0}")]
    Configuration(&'static str),
    #[error("invalid_request")]
    InvalidRequest,
    #[error("unauthorized_client")]
    UnauthorizedClient,
    #[error("invalid_grant")]
    InvalidGrant,
    #[error("invalid provider response")]
    Provider,
    #[error("issuer storage unavailable")]
    Storage,
    #[error("issuer database unavailable")]
    Database(#[from] rusqlite::Error),
    #[error("invalid issuer data")]
    Json(#[from] serde_json::Error),
    #[error("token verification failed")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("provider unavailable")]
    Http(#[from] reqwest::Error),
    #[error("account admission failed")]
    Admission(#[from] super::issuer::IssuerError),
}

/// Secrets deliberately have no Debug implementation.
pub struct IssuerConfig {
    pub issuer: String,
    pub signing_key_pem: Vec<u8>,
    pub key_id: String,
    /// Previously active public JWKs retained until their access tokens expire.
    pub retired_jwks: Vec<Value>,
    pub github: GithubConfig,
    pub google: GoogleConfig,
}

pub struct IssuerRuntime {
    pub(crate) config: IssuerConfig,
    pub(crate) store: Arc<dyn IssuerStore>,
    pub(crate) http: reqwest::Client,
    pub(crate) key: jsonwebtoken::EncodingKey,
    pub(crate) jwk: Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Authorization {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: String,
    challenge: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Flow {
    authorization: Authorization,
    provider: String,
    nonce: String,
    browser: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    subject: String,
    properties: Value,
    client_id: String,
    redirect_uri: String,
    challenge: Option<String>,
    next: String,
    used: Option<i64>,
}

pub struct BrowserResponse {
    pub location: Option<String>,
    pub html: Option<String>,
    pub cookie: Option<String>,
}

/// Account policy remains separate from cryptographic provider verification.
#[async_trait::async_trait]
pub trait AccountAdmission: Send + Sync {
    async fn admit(&self, identity: &ProviderIdentity) -> Result<IssuerOutcome, Error>;
}

pub struct TenantAdmission {
    pub tenant: crate::console::tenant::TenantStore,
    pub stage: String,
}

#[async_trait::async_trait]
impl AccountAdmission for TenantAdmission {
    async fn admit(&self, identity: &ProviderIdentity) -> Result<IssuerOutcome, Error> {
        Ok(super::issuer::issuer_success(&self.tenant, identity, &self.stage).await?)
    }
}

impl IssuerRuntime {
    pub fn new(config: IssuerConfig, store: Arc<dyn IssuerStore>) -> Result<Self, Error> {
        let issuer =
            url::Url::parse(&config.issuer).map_err(|_| Error::Configuration("issuer URL"))?;
        if issuer.origin().ascii_serialization() != config.issuer
            || !safe_endpoint(&config.issuer)
            || config.key_id.is_empty()
        {
            return Err(Error::Configuration(
                "issuer must be an HTTPS origin, or loopback fixture",
            ));
        }
        config.github.validate()?;
        config.google.validate()?;
        let mut key_ids = std::collections::HashSet::from([config.key_id.as_str()]);
        for retired in &config.retired_jwks {
            if ["d", "p", "q", "dp", "dq", "qi", "oth", "k"]
                .iter()
                .any(|name| retired.get(*name).is_some())
                || retired["kid"].as_str().is_none_or(str::is_empty)
                || retired["kid"].as_str() == Some(&config.key_id)
                || !((retired["alg"] == "ES256"
                    && retired["kty"] == "EC"
                    && retired["crv"] == "P-256")
                    || (retired["alg"] == "RS512" && retired["kty"] == "RSA"))
            {
                return Err(Error::Configuration(
                    "retired JWKS must contain distinct public ES256 or RS512 keys only",
                ));
            }
            if !key_ids.insert(
                retired["kid"]
                    .as_str()
                    .ok_or(Error::Configuration("retired key ID"))?,
            ) {
                return Err(Error::Configuration("duplicate signing key ID"));
            }
            let public: jsonwebtoken::jwk::Jwk = serde_json::from_value(retired.clone())?;
            jsonwebtoken::DecodingKey::from_jwk(&public)?;
        }
        let key = jsonwebtoken::EncodingKey::from_ec_pem(&config.signing_key_pem)?;
        let mut jwk = serde_json::to_value(jsonwebtoken::jwk::Jwk::from_encoding_key(
            &key,
            jsonwebtoken::Algorithm::ES256,
        )?)?;
        jwk["kid"] = json!(config.key_id);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            config,
            store,
            http,
            key,
            jwk,
        })
    }

    pub fn discovery(&self) -> Value {
        let issuer = &self.config.issuer;
        json!({"issuer": issuer, "authorization_endpoint":format!("{issuer}/authorize"), "token_endpoint":format!("{issuer}/token"), "jwks_uri":format!("{issuer}/.well-known/jwks.json"), "response_types_supported":["code","token"], "code_challenge_methods_supported":["S256"]})
    }

    pub fn jwks(&self) -> Value {
        let mut keys = vec![self.jwk.clone()];
        keys.extend(self.config.retired_jwks.clone());
        json!({"keys":keys})
    }

    pub async fn authorize(
        &self,
        query: &HashMap<String, String>,
    ) -> Result<BrowserResponse, Error> {
        let required = |name: &str| {
            query
                .get(name)
                .filter(|value| !value.is_empty())
                .cloned()
                .ok_or(Error::InvalidRequest)
        };
        let client_id = required("client_id")?;
        let redirect_uri = required("redirect_uri")?;
        if !is_allowed_authorization_redirect(&client_id, &redirect_uri) {
            return Err(Error::UnauthorizedClient);
        }
        let response_type = required("response_type")?;
        if response_type != "code" && response_type != "token" {
            return Err(Error::InvalidRequest);
        }
        let challenge = query.get("code_challenge").cloned();
        if challenge.as_ref().is_some_and(|value| {
            value.len() != 43
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        }) || challenge.is_some() != query.contains_key("code_challenge_method")
            || (challenge.is_some()
                && query.get("code_challenge_method").map(String::as_str) != Some("S256"))
        {
            return Err(Error::InvalidRequest);
        }
        let authorization = Authorization {
            client_id,
            redirect_uri,
            response_type,
            state: query.get("state").cloned().unwrap_or_default(),
            challenge,
        };
        let browser = random()?;
        self.store.put(
            &format!("browser:{browser}"),
            &serde_json::to_value(&authorization)?,
            now() + 86400,
        )?;
        let mut response = if let Some(provider) = query.get("provider") {
            self.start_provider(provider, &browser).await?
        } else {
            BrowserResponse { location: None, html: Some("<!doctype html><title>Sign in</title><img alt=\"OpenCode\" src=\"https://opencode.ai/favicon-v3.svg\"><a href=\"/github/authorize\">Continue with GitHub</a><a href=\"/google/authorize\">Continue with Google</a>".into()), cookie: None }
        };
        response.cookie = Some(self.browser_cookie(&browser, 86400));
        Ok(response)
    }

    pub fn browser_cookie(&self, value: &str, age: i64) -> String {
        format!(
            "{}={value}; Path=/; HttpOnly; Max-Age={age}{}",
            self.cookie_name(),
            if self.config.issuer.starts_with("https:") {
                "; Secure; SameSite=None"
            } else {
                "; SameSite=Lax"
            }
        )
    }

    pub fn cookie_name(&self) -> &'static str {
        if self.config.issuer.starts_with("https:") {
            "__Host-issuer_browser"
        } else {
            "issuer_browser"
        }
    }

    pub(crate) fn grant(
        &self,
        flow: &Flow,
        account_id: String,
        email: String,
    ) -> Result<BrowserResponse, Error> {
        let grant = Grant {
            properties: super::issuer::account_subject_properties(&account_id, &email),
            subject: account_id,
            client_id: flow.authorization.client_id.clone(),
            redirect_uri: flow.authorization.redirect_uri.clone(),
            challenge: flow.authorization.challenge.clone(),
            next: random()?,
            used: None,
        };
        let mut destination =
            url::Url::parse(&grant.redirect_uri).map_err(|_| Error::InvalidRequest)?;
        if flow.authorization.response_type == "token" {
            let tokens = self.issue(&grant)?;
            let fragment = url::form_urlencoded::Serializer::new(String::new())
                .append_pair(
                    "access_token",
                    tokens["access_token"].as_str().ok_or(Error::Storage)?,
                )
                .append_pair(
                    "refresh_token",
                    tokens["refresh_token"].as_str().ok_or(Error::Storage)?,
                )
                .append_pair("state", &flow.authorization.state)
                .finish();
            destination.set_fragment(Some(&fragment));
        } else {
            let code = random()?;
            self.store.put(
                &format!("code:{code}"),
                &serde_json::to_value(&grant)?,
                now() + 60,
            )?;
            let retained: Vec<(String, String)> = destination
                .query_pairs()
                .into_owned()
                .filter(|(name, _)| name != "code" && name != "state")
                .collect();
            destination.set_query(None);
            destination.query_pairs_mut().extend_pairs(retained);
            destination
                .query_pairs_mut()
                .append_pair("code", &code)
                .append_pair("state", &flow.authorization.state);
        }
        self.store
            .take(&format!("browser:{}", flow.browser), now())?;
        Ok(BrowserResponse {
            location: Some(destination.into()),
            html: None,
            cookie: Some(self.browser_cookie("", 0)),
        })
    }
}

pub(crate) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(crate) fn random() -> Result<String, Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Storage)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
pub(crate) fn refresh_key(subject: &str, token: &str) -> String {
    format!("refresh:{subject}:{token}")
}
pub(crate) fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
pub(crate) fn safe_endpoint(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && (url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
    })
}
