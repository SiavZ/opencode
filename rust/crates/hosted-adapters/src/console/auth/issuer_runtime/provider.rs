//! Purpose: GitHub OAuth and Google OIDC orchestration with bound one-use browser state.
//! Scope: provider HTTPS requests, loopback fixture endpoints allowed explicitly by config.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- jsonwebtoken::decode
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! Invariants: no subject admitted until signature or server-side GitHub token exchange verifies it.
//! Contract tests: issuer_runtime/tests.rs

use std::collections::HashMap;

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use opencode_hosted_domain::console::identity::{GithubEmail, IssuerOutcome, ProviderIdentity};
use serde_json::{Value, json};

use super::{
    AccountAdmission, BrowserResponse, Error, Flow, IssuerRuntime, now, random, safe_endpoint,
};

pub struct GithubConfig {
    pub client_id: String,
    pub client_secret: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub api_base: String,
}

impl GithubConfig {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.client_id.is_empty()
            || self.client_secret.is_empty()
            || ![
                &self.authorization_endpoint,
                &self.token_endpoint,
                &self.api_base,
            ]
            .iter()
            .all(|value| safe_endpoint(value))
        {
            return Err(Error::Configuration(
                "GitHub credentials/endpoints required",
            ));
        }
        Ok(())
    }
}

pub struct GoogleConfig {
    pub client_id: String,
    pub issuer: String,
}

impl GoogleConfig {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.client_id.is_empty() || !safe_endpoint(&self.issuer) {
            return Err(Error::Configuration("Google client ID/issuer required"));
        }
        Ok(())
    }
}

impl IssuerRuntime {
    async fn google_metadata(&self) -> Result<Value, Error> {
        let metadata: Value = self
            .http
            .get(format!(
                "{}/.well-known/openid-configuration",
                self.config.google.issuer.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if metadata["issuer"].as_str() != Some(self.config.google.issuer.as_str()) {
            return Err(Error::Provider);
        }
        for name in ["authorization_endpoint", "jwks_uri"] {
            if !metadata[name].as_str().is_some_and(safe_endpoint) {
                return Err(Error::Provider);
            }
        }
        Ok(metadata)
    }

    pub async fn start_provider(
        &self,
        provider: &str,
        browser: &str,
    ) -> Result<BrowserResponse, Error> {
        if provider != "github" && provider != "google" {
            return Err(Error::InvalidRequest);
        }
        let authorization = self
            .store
            .get(&format!("browser:{browser}"), now())?
            .ok_or(Error::InvalidGrant)?;
        let state = random()?;
        let flow = Flow {
            authorization: serde_json::from_value(authorization)?,
            provider: provider.into(),
            nonce: random()?,
            browser: browser.into(),
        };
        let endpoint = if provider == "github" {
            self.config.github.authorization_endpoint.clone()
        } else {
            self.google_metadata().await?["authorization_endpoint"]
                .as_str()
                .ok_or(Error::Provider)?
                .to_string()
        };
        let mut destination = url::Url::parse(&endpoint)
            .map_err(|_| Error::Configuration("provider authorization URL"))?;
        let callback = format!("{}/{provider}/callback", self.config.issuer);
        let mut query = destination.query_pairs_mut();
        query
            .append_pair("redirect_uri", &callback)
            .append_pair("state", &state);
        if provider == "github" {
            query
                .append_pair("client_id", &self.config.github.client_id)
                .append_pair("response_type", "code")
                .append_pair("scope", "read:user user:email");
        } else {
            query
                .append_pair("client_id", &self.config.google.client_id)
                .append_pair("response_type", "id_token")
                .append_pair("response_mode", "form_post")
                .append_pair("nonce", &flow.nonce)
                .append_pair("scope", "openid openid email");
        }
        drop(query);
        self.store.put(
            &format!("flow:{state}"),
            &serde_json::to_value(&flow)?,
            now() + 600,
        )?;
        Ok(BrowserResponse {
            location: Some(destination.into()),
            html: None,
            cookie: None,
        })
    }

    pub async fn callback(
        &self,
        provider: &str,
        values: &HashMap<String, String>,
        browser: &str,
        admission: &dyn AccountAdmission,
    ) -> Result<BrowserResponse, Error> {
        let flow = values
            .get("state")
            .map(|state| self.store.get(&format!("flow:{state}"), now()))
            .transpose()?
            .flatten()
            .map(serde_json::from_value::<Flow>)
            .transpose()?;
        let bound = flow.filter(|flow| {
            !browser.is_empty() && flow.browser == browser && flow.provider == provider
        });
        let result = self
            .callback_inner(provider, values, browser, admission)
            .await;
        let (Err(error), Some(flow)) = (&result, bound) else {
            return result;
        };
        let mut destination =
            url::Url::parse(&flow.authorization.redirect_uri).map_err(|_| Error::InvalidRequest)?;
        let provider_error = values.get("error").filter(|error| {
            matches!(
                error.as_str(),
                "access_denied" | "temporarily_unavailable" | "server_error" | "invalid_request"
            )
        });
        let (code, description) = if let Some(code) = provider_error {
            (
                code.as_str(),
                values
                    .get("error_description")
                    .map(String::as_str)
                    .unwrap_or("Provider authorization failed"),
            )
        } else {
            match error {
                Error::Provider | Error::Jwt(_) => {
                    ("invalid_request", "Provider identity verification failed")
                }
                _ => ("server_error", "Authentication failed"),
            }
        };
        let retained: Vec<(String, String)> = destination
            .query_pairs()
            .into_owned()
            .filter(|(name, _)| name != "error" && name != "error_description")
            .collect();
        destination.set_query(None);
        destination
            .query_pairs_mut()
            .extend_pairs(retained)
            .append_pair("error", code)
            .append_pair("error_description", description);
        Ok(BrowserResponse {
            location: Some(destination.into()),
            html: None,
            cookie: Some(self.browser_cookie("", 0)),
        })
    }

    async fn callback_inner(
        &self,
        provider: &str,
        values: &HashMap<String, String>,
        browser: &str,
        admission: &dyn AccountAdmission,
    ) -> Result<BrowserResponse, Error> {
        let state = values.get("state").ok_or(Error::InvalidRequest)?;
        // Include the unguessable browser cookie in the lookup checks before consumption.
        let key = format!("flow:{state}");
        let candidate: Flow =
            serde_json::from_value(self.store.get(&key, now())?.ok_or(Error::InvalidGrant)?)?;
        if candidate.provider != provider || candidate.browser != browser || browser.is_empty() {
            return Err(Error::InvalidGrant);
        }
        let flow: Flow =
            serde_json::from_value(self.store.take(&key, now())?.ok_or(Error::InvalidGrant)?)?;
        if self
            .store
            .take(&format!("browser:{browser}"), now())?
            .is_none()
        {
            return Err(Error::InvalidGrant);
        }
        if values.contains_key("error") {
            return Err(Error::Provider);
        }
        let identity = match provider {
            "github" => {
                self.verify_github(values.get("code").ok_or(Error::InvalidRequest)?)
                    .await?
            }
            "google" => {
                self.verify_google(
                    values.get("id_token").ok_or(Error::InvalidRequest)?,
                    &flow.nonce,
                )
                .await?
            }
            _ => return Err(Error::InvalidRequest),
        };
        match admission.admit(&identity).await? {
            IssuerOutcome::IssueAccount { account_id, email } => {
                self.grant(&flow, account_id, email)
            }
            IssuerOutcome::RedirectToNewConsole => {
                let mut destination = url::Url::parse(&self.config.issuer)
                    .map_err(|_| Error::Configuration("issuer"))?;
                let host = destination
                    .host_str()
                    .ok_or(Error::Configuration("issuer host"))?
                    .trim_start_matches("auth.")
                    .to_string();
                destination
                    .set_host(Some(&host))
                    .map_err(|_| Error::Configuration("console host"))?;
                destination.set_path("/console");
                self.store.take(&format!("browser:{browser}"), now())?;
                Ok(BrowserResponse {
                    location: Some(destination.into()),
                    html: None,
                    cookie: Some(self.browser_cookie("", 0)),
                })
            }
        }
    }

    async fn verify_github(&self, code: &str) -> Result<ProviderIdentity, Error> {
        let callback = format!("{}/github/callback", self.config.issuer);
        let tokens: Value = self
            .http
            .post(&self.config.github.token_endpoint)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", self.config.github.client_id.as_str()),
                ("client_secret", self.config.github.client_secret.as_str()),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", callback.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if tokens.get("error").is_some() {
            return Err(Error::Provider);
        }
        let access = tokens["access_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or(Error::Provider)?;
        let get = |path: &str| {
            self.http
                .get(format!(
                    "{}{path}",
                    self.config.github.api_base.trim_end_matches('/')
                ))
                .bearer_auth(access)
                .header("User-Agent", "opencode")
                .header("Accept", "application/vnd.github+json")
                .send()
        };
        let emails: Vec<GithubEmail> = get("/user/emails")
            .await?
            .error_for_status()?
            .json()
            .await?;
        let user: Value = get("/user").await?.error_for_status()?.json().await?;
        let user_id = user
            .get("id")
            .filter(|value| value.is_u64() || value.as_str().is_some_and(|id| !id.is_empty()))
            .cloned()
            .ok_or(Error::Provider)?;
        if !emails
            .iter()
            .any(|email| email.primary && email.verified && !email.email.is_empty())
        {
            return Err(Error::Provider);
        }
        Ok(ProviderIdentity::Github { user_id, emails })
    }

    async fn verify_google(&self, token: &str, nonce: &str) -> Result<ProviderIdentity, Error> {
        let metadata = self.google_metadata().await?;
        let keys: jsonwebtoken::jwk::JwkSet = self
            .http
            .get(metadata["jwks_uri"].as_str().ok_or(Error::Provider)?)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let header = decode_header(token)?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(Error::Provider);
        }
        let jwk = keys
            .find(header.kid.as_deref().ok_or(Error::Provider)?)
            .ok_or(Error::Provider)?;
        if let Some(algorithm) = &jwk.common.key_algorithm
            && serde_json::to_value(algorithm)? != serde_json::to_value(header.alg)?
        {
            return Err(Error::Provider);
        }
        if jwk
            .common
            .public_key_use
            .as_ref()
            .is_some_and(|usage| *usage != jsonwebtoken::jwk::PublicKeyUse::Signature)
        {
            return Err(Error::Provider);
        }
        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        validation.set_audience(&[&self.config.google.client_id]);
        validation.set_issuer(&[&self.config.google.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
        validation.validate_nbf = true;
        let claims = decode::<Value>(token, &DecodingKey::from_jwk(jwk)?, &validation)?.claims;
        if claims["nonce"].as_str() != Some(nonce)
            || claims["email_verified"] != json!(true)
            || claims["iat"]
                .as_i64()
                .is_none_or(|issued| issued > now() + 60)
            || (claims.get("azp").is_some()
                && claims["azp"].as_str() != Some(&self.config.google.client_id))
            || claims["aud"].as_array().is_some_and(|aud| {
                aud.len() > 1 && claims["azp"].as_str() != Some(&self.config.google.client_id)
            })
        {
            return Err(Error::Provider);
        }
        let sub = claims["sub"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or(Error::Provider)?
            .to_string();
        let email = claims["email"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or(Error::Provider)?
            .to_string();
        Ok(ProviderIdentity::Google {
            sub: Some(sub),
            email: Some(email),
            email_verified: true,
        })
    }
}
