//! Purpose: verify GitHub Actions OIDC ID tokens against the issuer JWKS and extract the
//! `repository` claim (`packages/function/src/api.ts:262-283`, `packages/function/src/github.ts:3-14`).
//! Mirrors `jose.jwtVerify(token, createRemoteJWKSet(..), { issuer, audience })`: iss and aud
//! are required and checked, exp/nbf are checked when present with zero clock tolerance, the
//! key is selected by `kid` (or by algorithm when the token has no kid, requiring exactly one
//! candidate), and an unknown kid triggers a rate-limited JWKS refetch to follow rotation.
//! Scope: process-wide verifier with a cached key set guarded by a mutex that is never held
//! across a network await.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/github/auth/mod.rs
//! External:
//! |-- jsonwebtoken::Algorithm
//! |-- jsonwebtoken::DecodingKey
//! |-- jsonwebtoken::Validation
//! |-- jsonwebtoken::decode
//! |-- jsonwebtoken::decode_header
//! |-- jsonwebtoken::jwk::AlgorithmParameters
//! |-- jsonwebtoken::jwk::Jwk
//! |-- jsonwebtoken::jwk::JwkSet
//! |-- jsonwebtoken::jwk::KeyOperations::Verify
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/github/auth/handlers.rs
//! |-- ecosystem/src/github/auth/mod.rs
//! |-- ecosystem/tests/github_auth_exchange.rs
//! `-- ecosystem/tests/github_auth_oidc.rs
//! Invariants: only RSA signature algorithms are accepted, `none`/HMAC are rejected, the
//! verifier never trusts an unverified claim.
//! Contract tests: ecosystem/tests/github_auth_oidc.rs

use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::{Map, Value};

use super::AuthError;

pub const GITHUB_ISSUER: &str = "https://token.actions.githubusercontent.com";
pub const EXPECTED_AUDIENCE: &str = "opencode-github-action";

const CACHE_MAX_AGE: Duration = Duration::from_secs(600);
const REFETCH_COOLDOWN: Duration = Duration::from_secs(30);
const RSA_ALGORITHMS: [Algorithm; 6] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
];

/// `{ owner, repo }` parsed from the `repository` claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryClaim {
    pub owner: String,
    pub repo: String,
}

/// Port of `parseRepositoryClaim`: exactly two non-empty `/`-separated parts.
pub fn parse_repository_claim(claims: &Map<String, Value>) -> Result<RepositoryClaim, AuthError> {
    let Some(Value::String(claim)) = claims.get("repository") else {
        return Err(AuthError::Invalid("Repository claim is missing".into()));
    };
    let parts: Vec<&str> = claim.split('/').collect();
    match parts.as_slice() {
        [owner, repo] if !owner.is_empty() && !repo.is_empty() => Ok(RepositoryClaim {
            owner: (*owner).to_string(),
            repo: (*repo).to_string(),
        }),
        _ => Err(AuthError::Invalid("Repository claim is invalid".into())),
    }
}

struct Cached {
    keys: JwkSet,
    fetched: Instant,
}

/// Verifies OIDC tokens for one issuer/audience pair.
pub struct OidcVerifier {
    jwks_url: String,
    issuer: String,
    audience: String,
    http: reqwest::Client,
    cache: Mutex<Option<Cached>>,
    refetch_cooldown: Duration,
}

impl std::fmt::Debug for OidcVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcVerifier")
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish()
    }
}

impl OidcVerifier {
    /// Verifier with the baseline GitHub issuer, audience and `${issuer}/.well-known/jwks`.
    pub fn github() -> Self {
        Self::new(
            format!("{GITHUB_ISSUER}/.well-known/jwks"),
            GITHUB_ISSUER,
            EXPECTED_AUDIENCE,
        )
    }

    /// Verifier with an injected JWKS URL (fixture servers) and explicit issuer/audience.
    pub fn new(
        jwks_url: impl Into<String>,
        issuer: impl Into<String>,
        audience: impl Into<String>,
    ) -> Self {
        Self {
            jwks_url: jwks_url.into(),
            issuer: issuer.into(),
            audience: audience.into(),
            http: reqwest::Client::new(),
            cache: Mutex::new(None),
            refetch_cooldown: REFETCH_COOLDOWN,
        }
    }

    /// Minimum age of the cached key set before an unknown `kid` triggers a refetch
    /// (jose `cooldownDuration`, 30s default). Tests shorten it to exercise rotation.
    pub fn with_refetch_cooldown(mut self, cooldown: Duration) -> Self {
        self.refetch_cooldown = cooldown;
        self
    }

    /// Verify the token and return its claims.
    pub async fn verify(&self, token: &str) -> Result<Map<String, Value>, AuthError> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| AuthError::Invalid(format!("invalid token header: {e}")))?;
        if !RSA_ALGORITHMS.contains(&header.alg) {
            return Err(AuthError::Invalid(format!(
                "unsupported token algorithm {:?}",
                header.alg
            )));
        }
        // No critical extensions are configured by the hosted route. JWT payloads must
        // remain base64url encoded, including when the RFC 7797 extension is present.
        if header.crit.as_ref().is_some_and(|fields| {
            fields.is_empty()
                || fields.iter().any(|field| field != "b64")
                || header.extras.inner().get("b64") != Some(&Value::Bool(true))
        }) {
            return Err(AuthError::Invalid(
                "unsupported critical token header".into(),
            ));
        }
        let jwk = self.select_key(header.kid.as_deref(), header.alg).await?;
        let key = DecodingKey::from_jwk(&jwk)
            .map_err(|e| AuthError::Invalid(format!("invalid JWK: {e}")))?;

        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        // Validate NumericDate directly below: jose accepts fractional/negative numbers,
        // while jsonwebtoken uses unsigned timestamps and a different expiry boundary.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.required_spec_claims = ["iss", "aud"].iter().map(|c| c.to_string()).collect();
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        let data = jsonwebtoken::decode::<Map<String, Value>>(token, &key, &validation)
            .map_err(|e| AuthError::Invalid(format!("token verification failed: {e}")))?;
        if data.claims.get("iss").and_then(Value::as_str) != Some(self.issuer.as_str()) {
            return Err(AuthError::Invalid("unexpected iss claim value".into()));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AuthError::Config("system clock precedes unix epoch".into()))?
            .as_secs() as f64;
        for field in ["iat", "exp", "nbf"] {
            if let Some(value) = data.claims.get(field) {
                let timestamp = value
                    .as_f64()
                    .ok_or_else(|| AuthError::Invalid(format!("{field} claim must be a number")))?;
                if (field == "exp" && timestamp <= now) || (field == "nbf" && timestamp > now) {
                    return Err(AuthError::Invalid(format!(
                        "{field} claim timestamp check failed"
                    )));
                }
            }
        }
        Ok(data.claims)
    }

    /// Verify and parse the repository claim in one step (the exchange route's needs).
    pub async fn verify_repository(&self, token: &str) -> Result<RepositoryClaim, AuthError> {
        parse_repository_claim(&self.verify(token).await?)
    }

    async fn select_key(&self, kid: Option<&str>, alg: Algorithm) -> Result<Jwk, AuthError> {
        let (cached, stale, may_refetch) = {
            let guard = self.cache.lock().expect("jwks cache poisoned");
            match guard.as_ref() {
                Some(c) => (
                    Some(c.keys.clone()),
                    c.fetched.elapsed() > CACHE_MAX_AGE,
                    c.fetched.elapsed() >= self.refetch_cooldown,
                ),
                None => (None, true, true),
            }
        };
        if let Some(keys) = cached.filter(|_| !stale) {
            match pick(&keys, kid, alg) {
                Ok(jwk) => return Ok(jwk),
                Err(e) if !may_refetch => return Err(e),
                Err(_) => {}
            }
        }
        let keys = self.fetch().await?;
        let picked = pick(&keys, kid, alg);
        *self.cache.lock().expect("jwks cache poisoned") = Some(Cached {
            keys,
            fetched: Instant::now(),
        });
        picked
    }

    async fn fetch(&self) -> Result<JwkSet, AuthError> {
        let response = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::Transport(format!("JWKS fetch failed: {e}")))?;
        if !response.status().is_success() {
            return Err(AuthError::Upstream {
                status: response.status().as_u16(),
                message: "Expected 200 OK from the JSON Web Key Set HTTP response".into(),
            });
        }
        response
            .json::<JwkSet>()
            .await
            .map_err(|e| AuthError::Invalid(format!("JSON Web Key Set malformed: {e}")))
    }
}

/// jose `LocalJWKSet` selection: filter by kid (when present), RSA key type, alg and
/// signature use, then require exactly one candidate.
fn pick(keys: &JwkSet, kid: Option<&str>, alg: Algorithm) -> Result<Jwk, AuthError> {
    let candidates: Vec<&Jwk> = keys
        .keys
        .iter()
        .filter(|k| kid.is_none() || k.common.key_id.as_deref() == kid)
        .filter(|k| matches!(k.algorithm, AlgorithmParameters::RSA(_)))
        .filter(|k| {
            k.common
                .key_algorithm
                .is_none_or(|a| format!("{a:?}") == format!("{alg:?}"))
        })
        .filter(|k| {
            k.common
                .public_key_use
                .as_ref()
                .is_none_or(|u| matches!(u, jsonwebtoken::jwk::PublicKeyUse::Signature))
        })
        .filter(|k| {
            k.common
                .key_operations
                .as_ref()
                .is_none_or(|ops| ops.contains(&jsonwebtoken::jwk::KeyOperations::Verify))
        })
        .collect();
    match candidates.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(AuthError::Invalid(
            "no applicable key found in the JSON Web Key Set".into(),
        )),
        _ => Err(AuthError::Invalid(
            "multiple matching keys found in the JSON Web Key Set".into(),
        )),
    }
}
