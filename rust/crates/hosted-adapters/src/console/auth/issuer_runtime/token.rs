//! Purpose: OpenAuth token wire contract, one-use code exchange and bounded refresh reuse.
//! Scope: issuer token and userinfo endpoints.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! External:
//! |-- jsonwebtoken::encode
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! Invariants: client and redirect bind codes; only S256 PKCE accepted; signing keys explicit.
//! Contract tests: issuer_runtime/tests.rs

use super::{
    ACCESS_TTL, Error, Grant, IssuerRuntime, REFRESH_TTL, digest, now, random, refresh_key,
};
use jsonwebtoken::{Algorithm, DecodingKey, Header, Validation, decode, encode};
use serde_json::{Value, json};
use std::collections::HashMap;

impl IssuerRuntime {
    pub fn token(&self, form: &HashMap<String, String>) -> Result<Value, Error> {
        let required = |name: &str| {
            form.get(name)
                .filter(|value| !value.is_empty())
                .ok_or(Error::InvalidRequest)
        };
        match required("grant_type")?.as_str() {
            "authorization_code" => {
                let code = required("code").map_err(|_| Error::OAuth {
                    code: "invalid_request",
                    description: "Missing code",
                    status: 400,
                })?;
                let grant: Grant = serde_json::from_value(
                    self.store
                        .take(&format!("code:{code}"), now())?
                        .ok_or(Error::OAuth {
                            code: "invalid_grant",
                            description: "Authorization code has been used or expired",
                            status: 400,
                        })?,
                )?;
                if form.get("redirect_uri") != Some(&grant.redirect_uri) {
                    return Err(Error::OAuth {
                        code: "invalid_redirect_uri",
                        description: "Redirect URI mismatch",
                        status: 400,
                    });
                }
                if form.get("client_id") != Some(&grant.client_id) {
                    return Err(Error::OAuth {
                        code: "unauthorized_client",
                        description: "Client is not authorized to use this authorization code",
                        status: 403,
                    });
                }
                if let Some(challenge) = &grant.challenge {
                    let verifier = required("code_verifier").map_err(|_| Error::OAuth {
                        code: "invalid_grant",
                        description: "Missing code_verifier",
                        status: 400,
                    })?;
                    if !(43..=128).contains(&verifier.len())
                        || !verifier
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
                        || digest(verifier) != *challenge
                    {
                        return Err(Error::OAuth {
                            code: "invalid_grant",
                            description: "Code verifier does not match",
                            status: 400,
                        });
                    }
                }
                self.issue(&grant)
            }
            "refresh_token" => {
                let (subject, token) = required("refresh_token")
                    .map_err(|_| Error::OAuth {
                        code: "invalid_request",
                        description: "Missing refresh_token",
                        status: 400,
                    })?
                    .rsplit_once(':')
                    .ok_or(Error::InvalidGrant)?;
                let key = refresh_key(subject, token);
                let candidate: Grant = serde_json::from_value(
                    self.store.get(&key, now())?.ok_or(Error::InvalidGrant)?,
                )?;
                if form
                    .get("client_id")
                    .is_some_and(|client| *client != candidate.client_id)
                {
                    return Err(Error::UnauthorizedClient);
                }
                let grant = self.store.refresh(&key, now()).map_err(|error| {
                    if matches!(error, Error::InvalidGrant) {
                        Error::OAuth {
                            code: "invalid_grant",
                            description: "Refresh token has been used or expired",
                            status: 400,
                        }
                    } else {
                        error
                    }
                })?;
                self.tokens(&grant, &grant.next, grant.used.unwrap_or_else(now))
            }
            _ => Err(Error::InvalidRequest),
        }
    }

    pub(crate) fn issue(&self, grant: &Grant) -> Result<Value, Error> {
        let refresh = random()?;
        self.store.put(
            &refresh_key(&grant.subject, &refresh),
            &serde_json::to_value(grant)?,
            now() + REFRESH_TTL,
        )?;
        self.tokens(grant, &refresh, now())
    }

    fn tokens(&self, grant: &Grant, refresh: &str, issued: i64) -> Result<Value, Error> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());
        header.typ = Some("JWT".into());
        let access = encode(
            &header,
            &json!({"mode":"access", "type":"account", "properties":grant.properties, "aud":grant.client_id, "iss":self.config.issuer, "sub":grant.subject, "exp":issued+ACCESS_TTL}),
            &self.key,
        )?;
        Ok(
            json!({"access_token":access,"refresh_token":format!("{}:{refresh}", grant.subject),"expires_in":issued+ACCESS_TTL-now()}),
        )
    }

    pub fn userinfo(&self, bearer: &str) -> Result<Value, Error> {
        let header = jsonwebtoken::decode_header(bearer)?;
        if !matches!(header.alg, Algorithm::ES256 | Algorithm::RS512) {
            return Err(Error::InvalidGrant);
        }
        let keys = self.jwks();
        let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(
            keys["keys"]
                .as_array()
                .ok_or(Error::Storage)?
                .iter()
                .find(|key| key["kid"].as_str() == header.kid.as_deref())
                .ok_or(Error::InvalidGrant)?
                .clone(),
        )?;
        if jwk.common.key_algorithm.as_ref().is_none_or(|alg| {
            serde_json::to_value(alg).ok() != serde_json::to_value(header.alg).ok()
        }) {
            return Err(Error::InvalidGrant);
        }
        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&["app"]);
        let claims = decode::<Value>(bearer, &DecodingKey::from_jwk(&jwk)?, &validation)?.claims;
        if claims["mode"] != "access"
            || claims["type"] != "account"
            || !claims["properties"]["accountID"].is_string()
            || !claims["properties"]["email"].is_string()
        {
            return Err(Error::InvalidGrant);
        }
        Ok(claims["properties"].clone())
    }
}
