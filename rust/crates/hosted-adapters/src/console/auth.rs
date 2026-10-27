//! Purpose: console authentication adapters: OpenAuth client contract (authorize URL, code
//! exchange, access-token subject decode), console `auth` session cookie (h3/iron sealed), the
//! issuer success flow (verified identity -> links -> account subject or new-console redirect)
//! and workspace actor admission against MySQL.
//! Scope: per-request adapter calls; HTTP via reqwest to the configured issuer/provider bases.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/iron.rs
//! |-- hosted-adapters/src/console/auth/issuer.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! |-- form_urlencoded::Serializer::new
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- time::OffsetDateTime
//! |-- time::OffsetDateTime::UNIX_EPOCH
//! |-- time::OffsetDateTime::from_unix_timestamp_nanos
//! |-- url::Url::parse
//! `-- uuid::Uuid::new_v4
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/mod.rs
//! `-- hosted-adapters/tests/console_identity_auth.rs
//! Invariants: client id is `app`; authorize uses `response_type=code` with random UUID state
//! stored in Redis for ten minutes, session-bound and atomically consumed once. No PKCE;
//! exchange posts an empty `code_verifier`. Access JWTs require trusted issuer discovery/JWKS,
//! an ES256/RS512 signature and validated issuer, audience, expiry and account subject claims.
//! Cookie name `auth`, maxAge one year, `HttpOnly`, not `Secure`
//! (`app/src/context/auth.ts:30-40`).
//! Contract tests: hosted-adapters/src/console/auth/iron.rs, hosted-adapters/tests/console_identity_auth.rs

pub mod iron;
pub mod issuer;
pub mod issuer_runtime;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use opencode_hosted_domain::console::common::{Actor, ConsoleError};
use opencode_hosted_domain::console::identity::{
    AccountSelection, Admission, AuthSession, admit_workspace, new_console_redirect,
    select_session_account,
};
use sha2::Digest;

use super::tenant::{TenantError, TenantStore};

pub const CLIENT_ID: &str = "app";
pub const SESSION_COOKIE: &str = "auth";
pub const SESSION_MAX_AGE_S: i64 = 60 * 60 * 24 * 365;

/// OpenAuth client errors with the baseline messages.
#[derive(Debug, thiserror::Error)]
pub enum AuthClientError {
    #[error("Invalid authorization code")]
    InvalidAuthorizationCode,
    #[error("Invalid access token")]
    InvalidAccessToken,
    #[error("issuer unavailable")]
    Transport,
    #[error("OAuth state store unavailable")]
    StateStoreUnavailable,
    #[error("Invalid authorization state")]
    InvalidState,
}

/// Decoded `account` subject from the access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSubject {
    pub account_id: String,
    pub email: String,
}

/// OpenAuth client bound to an issuer base URL.
#[derive(Clone, Debug)]
pub struct AuthClient {
    issuer: String,
    http: reqwest::Client,
    state_store: Option<OAuthStateStore>,
}

#[derive(Clone, Debug)]
enum OAuthStateStore {
    Redis(redis::Client),
}

#[derive(Clone)]
struct IssuerKey {
    jwk: jsonwebtoken::jwk::Jwk,
    expires_at: Option<u64>,
}

struct CachedJwks {
    fetched_at: Instant,
    keys: Vec<IssuerKey>,
}

#[derive(serde::Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(serde::Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct AccessClaims {
    iss: String,
    aud: String,
    exp: u64,
    sub: String,
    mode: String,
    #[serde(rename = "type")]
    subject_type: String,
    properties: AccountProperties,
}

#[derive(serde::Deserialize)]
struct AccountProperties {
    #[serde(rename = "accountID")]
    account_id: String,
    email: String,
}

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
const JWKS_CACHE_CAPACITY: usize = 64;
const MAX_ISSUER_DOCUMENT_BYTES: usize = 262_144;
const MAX_JWKS_KEYS: usize = 64;
const OAUTH_STATE_TTL_SECONDS: u64 = 600;

fn jwks_cache() -> &'static tokio::sync::Mutex<HashMap<String, CachedJwks>> {
    static CACHE: OnceLock<tokio::sync::Mutex<HashMap<String, CachedJwks>>> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

impl AuthClient {
    pub fn new(issuer: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            issuer: issuer.into().trim_end_matches('/').to_string(),
            http,
            state_store: None,
        }
    }

    /// Uses Redis for shared, expiring, one-time OAuth state claims.
    pub fn with_state_store(mut self, redis_url: &str) -> Result<Self, AuthClientError> {
        self.state_store = Some(OAuthStateStore::Redis(
            redis::Client::open(redis_url).map_err(|_| AuthClientError::StateStoreUnavailable)?,
        ));
        Ok(self)
    }

    /// Issues a fresh random OAuth state value bound to the current sealed-session id.
    pub async fn create_oauth_state(&self, session_id: &str) -> Result<String, AuthClientError> {
        let store = self
            .state_store
            .as_ref()
            .ok_or(AuthClientError::StateStoreUnavailable)?;
        let binding = hex::encode(sha2::Sha256::digest(session_id.as_bytes()));
        for _ in 0..3 {
            let state = uuid::Uuid::new_v4().to_string();
            let key = oauth_state_key(&state);
            let stored = match store {
                OAuthStateStore::Redis(client) => {
                    let mut connection = client
                        .get_multiplexed_async_connection()
                        .await
                        .map_err(|_| AuthClientError::StateStoreUnavailable)?;
                    redis::cmd("SET")
                        .arg(&key)
                        .arg(&binding)
                        .arg("NX")
                        .arg("EX")
                        .arg(OAUTH_STATE_TTL_SECONDS)
                        .query_async::<Option<String>>(&mut connection)
                        .await
                        .map_err(|_| AuthClientError::StateStoreUnavailable)?
                        .is_some()
                }
            };
            if stored {
                return Ok(state);
            }
        }
        Err(AuthClientError::StateStoreUnavailable)
    }

    /// Atomically consumes state only for the session that created it.
    pub async fn consume_oauth_state(
        &self,
        state: &str,
        session_id: &str,
    ) -> Result<(), AuthClientError> {
        let store = self
            .state_store
            .as_ref()
            .ok_or(AuthClientError::StateStoreUnavailable)?;
        if uuid::Uuid::parse_str(state).is_err() || session_id.is_empty() {
            return Err(AuthClientError::InvalidState);
        }
        let binding = hex::encode(sha2::Sha256::digest(session_id.as_bytes()));
        let key = oauth_state_key(state);
        let valid = match store {
            OAuthStateStore::Redis(client) => {
                let mut connection = client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|_| AuthClientError::StateStoreUnavailable)?;
                redis::cmd("EVAL")
                    .arg("local v=redis.call('GET',KEYS[1]); if not v then return 0 end; if v~=ARGV[1] then return -1 end; redis.call('DEL',KEYS[1]); return 1")
                    .arg(1)
                    .arg(key)
                    .arg(binding)
                    .query_async::<i32>(&mut connection)
                    .await
                    .map_err(|_| AuthClientError::StateStoreUnavailable)?
                    == 1
            }
        };
        if valid {
            return Ok(());
        }
        Err(AuthClientError::InvalidState)
    }

    /// `AuthClient.authorize(redirectURI, "code")`.
    pub fn authorize_url(&self, redirect_uri: &str, state: &str) -> String {
        let mut url = url::Url::parse(&format!("{}/authorize", self.issuer))
            .unwrap_or_else(|_| url::Url::parse("http://invalid/authorize").expect("static url"));
        url.query_pairs_mut()
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("state", state);
        url.to_string()
    }

    /// `AuthClient.exchange(code, redirectURI)`: returns the access token.
    pub async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<String, AuthClientError> {
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("code", code)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("grant_type", "authorization_code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("code_verifier", "")
            .finish();
        let res = self
            .http
            .post(format!("{}/token", self.issuer))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| AuthClientError::Transport)?;
        let ok = res.status().is_success();
        let json: serde_json::Value = res.json().await.map_err(|_| AuthClientError::Transport)?;
        if !ok {
            return Err(AuthClientError::InvalidAuthorizationCode);
        }
        let token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|token| !token.is_empty())
            .ok_or(AuthClientError::InvalidAccessToken)?;
        Ok(token.to_string())
    }

    /// Verifies the issuer JWT and extracts the account subject.
    pub async fn verify_account_subject(
        &self,
        token: &str,
    ) -> Result<AccountSubject, AuthClientError> {
        let issuer = self.issuer_origin()?;
        let header =
            jsonwebtoken::decode_header(token).map_err(|_| AuthClientError::InvalidAccessToken)?;
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or(AuthClientError::InvalidAccessToken)?;
        let algorithm = match header.alg {
            jsonwebtoken::Algorithm::ES256 | jsonwebtoken::Algorithm::RS512 => header.alg,
            _ => return Err(AuthClientError::InvalidAccessToken),
        };
        if header.typ.as_deref() != Some("JWT") {
            return Err(AuthClientError::InvalidAccessToken);
        }

        let mut keys = self.issuer_keys(&issuer, false).await?;
        let key = matching_issuer_key(&keys, kid, algorithm);
        let key = match key {
            Some(key) => key,
            None => {
                keys = self.issuer_keys(&issuer, true).await?;
                matching_issuer_key(&keys, kid, algorithm)
                    .ok_or(AuthClientError::InvalidAccessToken)?
            }
        };
        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(&key.jwk)
            .map_err(|_| AuthClientError::InvalidAccessToken)?;
        let mut validation = jsonwebtoken::Validation::new(algorithm);
        validation.leeway = 0;
        validation.set_issuer(&[issuer.as_str()]);
        validation.set_audience(&[CLIENT_ID]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = jsonwebtoken::decode::<AccessClaims>(token, &decoding_key, &validation)
            .map_err(|_| AuthClientError::InvalidAccessToken)?
            .claims;
        if claims.iss != issuer
            || claims.aud != CLIENT_ID
            || claims.exp <= unix_now()
            || claims.sub.is_empty()
            || claims.mode != "access"
            || claims.subject_type != "account"
            || claims.properties.account_id.is_empty()
            || claims.properties.email.is_empty()
        {
            return Err(AuthClientError::InvalidAccessToken);
        }
        Ok(AccountSubject {
            account_id: claims.properties.account_id,
            email: claims.properties.email,
        })
    }

    async fn issuer_keys(
        &self,
        issuer: &str,
        force_refresh: bool,
    ) -> Result<Vec<IssuerKey>, AuthClientError> {
        if !force_refresh
            && let Some(cache) = jwks_cache().lock().await.get(issuer)
            && cache.fetched_at.elapsed() < JWKS_CACHE_TTL
        {
            return Ok(cache.keys.clone());
        }

        let discovery_url = format!("{issuer}/.well-known/oauth-authorization-server");
        let response = self
            .http
            .get(discovery_url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| AuthClientError::Transport)?;
        if !response.status().is_success() {
            return Err(AuthClientError::Transport);
        }
        let discovery = serde_json::from_slice::<Discovery>(&read_limited(response).await?)
            .map_err(|_| AuthClientError::Transport)?;
        if discovery.issuer != issuer {
            return Err(AuthClientError::InvalidAccessToken);
        }
        let jwks_url =
            url::Url::parse(&discovery.jwks_uri).map_err(|_| AuthClientError::Transport)?;
        if !is_trusted_issuer_url(&jwks_url) || jwks_url.origin().ascii_serialization() != issuer {
            return Err(AuthClientError::InvalidAccessToken);
        }
        let response = self
            .http
            .get(jwks_url.as_str())
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| AuthClientError::Transport)?;
        if !response.status().is_success() {
            return Err(AuthClientError::Transport);
        }
        let document = serde_json::from_slice::<Jwks>(&read_limited(response).await?)
            .map_err(|_| AuthClientError::Transport)?;
        if document.keys.len() > MAX_JWKS_KEYS {
            return Err(AuthClientError::Transport);
        }
        let keys = document
            .keys
            .into_iter()
            .filter_map(|value| {
                let expires_at = value.get("exp").and_then(serde_json::Value::as_u64);
                let jwk = serde_json::from_value(value).ok()?;
                Some(IssuerKey { jwk, expires_at })
            })
            .collect::<Vec<_>>();
        let mut cache = jwks_cache().lock().await;
        cache.retain(|_, entry| entry.fetched_at.elapsed() < JWKS_CACHE_TTL);
        if cache.len() >= JWKS_CACHE_CAPACITY
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at)
                .map(|(issuer, _)| issuer.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(
            issuer.to_string(),
            CachedJwks {
                fetched_at: Instant::now(),
                keys: keys.clone(),
            },
        );
        Ok(keys)
    }

    fn issuer_origin(&self) -> Result<String, AuthClientError> {
        let issuer = url::Url::parse(&self.issuer).map_err(|_| AuthClientError::Transport)?;
        if !is_trusted_issuer_url(&issuer)
            || !matches!(issuer.path(), "" | "/")
            || issuer.query().is_some()
            || issuer.fragment().is_some()
        {
            return Err(AuthClientError::Transport);
        }
        Ok(issuer.origin().ascii_serialization())
    }
}

fn oauth_state_key(state: &str) -> String {
    format!(
        "opencode:console:oauth-state:{}",
        hex::encode(sha2::Sha256::digest(state.as_bytes()))
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn is_trusted_issuer_url(url: &url::Url) -> bool {
    if url.username() != "" || url.password().is_some() {
        return false;
    }
    if url.scheme() == "https" {
        return url.host_str().is_some();
    }
    url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

fn matching_issuer_key<'a>(
    keys: &'a [IssuerKey],
    kid: &str,
    algorithm: jsonwebtoken::Algorithm,
) -> Option<&'a IssuerKey> {
    let now = unix_now();
    let matches = keys.iter().filter(|key| {
        let common = &key.jwk.common;
        common.key_id.as_deref() == Some(kid)
            && common.key_algorithm.is_some_and(|key_algorithm| {
                jsonwebtoken::Algorithm::try_from(key_algorithm).ok() == Some(algorithm)
            })
            && common
                .public_key_use
                .as_ref()
                .is_none_or(|usage| *usage == jsonwebtoken::jwk::PublicKeyUse::Signature)
            && common.key_operations.as_ref().is_none_or(|operations| {
                operations.contains(&jsonwebtoken::jwk::KeyOperations::Verify)
            })
            && key.expires_at.is_none_or(|expires_at| expires_at > now)
    });
    let mut matches = matches;
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, AuthClientError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AuthClientError::Transport)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_ISSUER_DOCUMENT_BYTES {
            return Err(AuthClientError::Transport);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Session cookie codec (`useSession({ password, name: "auth", maxAge: 1 year })`).
#[derive(Clone)]
pub struct SessionCodec {
    password: String,
}

impl std::fmt::Debug for SessionCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCodec").finish_non_exhaustive()
    }
}

/// Loaded session: envelope id/createdAt plus data.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSession {
    pub id: String,
    pub created_at: i64,
    pub data: AuthSession,
}

impl SessionCodec {
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
        }
    }

    /// Reads the sealed cookie; invalid or expired seals yield a fresh session (h3 behavior).
    pub fn load(&self, sealed: Option<&str>, now_ms: i64) -> LoadedSession {
        sealed
            .and_then(|s| {
                iron::unseal_session::<AuthSession>(s, &self.password, SESSION_MAX_AGE_S, now_ms)
                    .ok()
            })
            .filter(|s| !s.id.is_empty())
            .map(|s| LoadedSession {
                id: s.id,
                created_at: s.created_at,
                data: s.data,
            })
            .unwrap_or_else(|| LoadedSession {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: now_ms,
                data: AuthSession::default(),
            })
    }

    /// Seals the session and returns the full `Set-Cookie` value.
    pub fn set_cookie(
        &self,
        session: &LoadedSession,
        now_ms: i64,
    ) -> Result<String, iron::IronError> {
        let sealed = iron::seal_session(
            &iron::H3Session {
                id: session.id.clone(),
                created_at: session.created_at,
                data: &session.data,
            },
            &self.password,
            SESSION_MAX_AGE_S,
            now_ms,
            iron::SealRandom::os(),
        )?;
        let expires = time::OffsetDateTime::from_unix_timestamp_nanos(
            (session.created_at as i128 + SESSION_MAX_AGE_S as i128 * 1000) * 1_000_000,
        )
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        Ok(format!(
            "{SESSION_COOKIE}={sealed}; Path=/; Expires={}; HttpOnly",
            http_date(expires)
        ))
    }
}

fn http_date(t: time::OffsetDateTime) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAYS[t.weekday().number_days_from_monday() as usize],
        t.day(),
        MONTHS[u8::from(t.month()) as usize - 1],
        t.year(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Actor resolution outcome for console requests (`getActor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorOutcome {
    Actor(Actor),
    /// Persist the session (current account changed) and use this actor.
    ActorWithSessionUpdate(Actor, AuthSession),
    Redirect(opencode_hosted_domain::console::identity::Redirect),
}

/// `getActor(workspace?)` against MySQL.
pub async fn resolve_actor(
    store: &TenantStore,
    session: &AuthSession,
    workspace: Option<&str>,
    method: &str,
    console_url: Option<&str>,
) -> Result<ActorOutcome, TenantError> {
    let Some(workspace) = workspace else {
        return match select_session_account(session) {
            AccountSelection::Public => Ok(ActorOutcome::Actor(Actor::Public {})),
            AccountSelection::Account {
                account,
                update_current,
            } => {
                if !store.account_has_black_membership(&account.id).await? {
                    return Ok(ActorOutcome::Redirect(new_console_redirect(console_url)?));
                }
                let actor = Actor::Account {
                    account_id: account.id.clone(),
                    email: account.email,
                };
                if !update_current {
                    return Ok(ActorOutcome::Actor(actor));
                }
                let mut next = session.clone();
                next.current = Some(account.id);
                Ok(ActorOutcome::ActorWithSessionUpdate(actor, next))
            }
        };
    };
    let accounts = session.account_ids();
    let black = store.black_accounts(&accounts).await?;
    let member = store.admission_member(workspace, &black).await?;
    match admit_workspace(
        !accounts.is_empty(),
        &black,
        member.as_ref(),
        method,
        console_url,
    )? {
        Admission::Redirect(r) => Ok(ActorOutcome::Redirect(r)),
        Admission::Actor(actor) => {
            if let Some(m) = &member {
                store.touch_seen(workspace, &m.user_id).await?;
            }
            Ok(ActorOutcome::Actor(actor))
        }
    }
}

impl From<ConsoleError> for AuthClientError {
    fn from(_: ConsoleError) -> Self {
        AuthClientError::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::get;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    const SIGNING_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgw7tDBuHtqIOB6FRR\nzJbG5AHmjP2dmnYddCN/3nE8RduhRANCAARPuKdPr3mreSJ9kiLkcz+02BWgFnA0\nsa2VMbNAn76sI5Wg4dQ2tB5AE9jAUwqZ1OnKSSstI9BcQODDGOngmrWI\n-----END PRIVATE KEY-----\n";

    fn signed_token(issuer: &str, audience: &str, exp: u64) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("controlled-es256".into());
        jsonwebtoken::encode(
            &header,
            &json!({
                "iss": issuer,
                "aud": audience,
                "exp": exp,
                "sub": "account:fixture",
                "mode": "access",
                "type": "account",
                "properties": {
                    "accountID": "acc_fixture",
                    "email": "fixture@example.test"
                }
            }),
            &EncodingKey::from_ec_pem(SIGNING_KEY).unwrap(),
        )
        .unwrap()
    }

    async fn serve_issuer() -> (String, tokio::task::JoinHandle<()>) {
        let key = EncodingKey::from_ec_pem(SIGNING_KEY).unwrap();
        let mut jwk = jsonwebtoken::jwk::Jwk::from_encoding_key(&key, Algorithm::ES256).unwrap();
        jwk.common.key_id = Some("controlled-es256".into());
        jwk.common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::ES256);
        jwk.common.public_key_use = Some(jsonwebtoken::jwk::PublicKeyUse::Signature);
        let jwks = Json(json!({ "keys": [jwk] }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let discovery_issuer = issuer.clone();
        let app = axum::Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(move || {
                    let issuer = discovery_issuer.clone();
                    async move {
                        Json(json!({
                            "issuer": issuer,
                            "jwks_uri": format!("{issuer}/.well-known/jwks.json")
                        }))
                    }
                }),
            )
            .route("/.well-known/jwks.json", get(move || async move { jwks }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, task)
    }

    #[tokio::test]
    async fn verifies_controlled_issuer_signature_and_required_claims() {
        let (issuer, server) = serve_issuer().await;
        let client = AuthClient::new(&issuer, reqwest::Client::new());
        let exp = unix_now() + 300;
        let valid = signed_token(&issuer, CLIENT_ID, exp);
        assert_eq!(
            client.verify_account_subject(&valid).await.unwrap(),
            AccountSubject {
                account_id: "acc_fixture".into(),
                email: "fixture@example.test".into(),
            }
        );

        let mut invalid_signature = valid.clone();
        let last = invalid_signature.pop().unwrap();
        invalid_signature.push(if last == 'A' { 'B' } else { 'A' });
        assert!(matches!(
            client.verify_account_subject(&invalid_signature).await,
            Err(AuthClientError::InvalidAccessToken)
        ));
        assert!(matches!(
            client
                .verify_account_subject(&signed_token("http://wrong-issuer", CLIENT_ID, exp))
                .await,
            Err(AuthClientError::InvalidAccessToken)
        ));
        assert!(matches!(
            client
                .verify_account_subject(&signed_token(&issuer, "another-client", exp))
                .await,
            Err(AuthClientError::InvalidAccessToken)
        ));
        assert!(matches!(
            client
                .verify_account_subject(&signed_token(&issuer, CLIENT_ID, unix_now() - 1))
                .await,
            Err(AuthClientError::InvalidAccessToken)
        ));
        server.abort();
    }

    async fn serve_redis_fixture() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let values = Arc::new(tokio::sync::Mutex::new(HashMap::<String, String>::new()));
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let values = values.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    while let Some(command) = read_resp_command(&mut reader).await {
                        let response = match command.first().map(String::as_str) {
                            Some("SET") if command.len() >= 3 => {
                                let mut values = values.lock().await;
                                if values.contains_key(&command[1]) {
                                    b"$-1\r\n".to_vec()
                                } else {
                                    values.insert(command[1].clone(), command[2].clone());
                                    b"+OK\r\n".to_vec()
                                }
                            }
                            Some("EVAL") if command.len() >= 5 => {
                                let mut values = values.lock().await;
                                match values.get(&command[3]) {
                                    None => b":0\r\n".to_vec(),
                                    Some(binding) if binding == &command[4] => {
                                        values.remove(&command[3]);
                                        b":1\r\n".to_vec()
                                    }
                                    Some(_) => b":-1\r\n".to_vec(),
                                }
                            }
                            _ => b"+OK\r\n".to_vec(),
                        };
                        if reader.get_mut().write_all(&response).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (format!("redis://{address}/?protocol=resp2"), task)
    }

    async fn read_resp_command(
        reader: &mut BufReader<tokio::net::TcpStream>,
    ) -> Option<Vec<String>> {
        let mut line = String::new();
        reader.read_line(&mut line).await.ok()?;
        let count = line.strip_prefix('*')?.trim().parse::<usize>().ok()?;
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            line.clear();
            reader.read_line(&mut line).await.ok()?;
            let length = line.strip_prefix('$')?.trim().parse::<usize>().ok()?;
            let mut bytes = vec![0; length + 2];
            reader.read_exact(&mut bytes).await.ok()?;
            args.push(String::from_utf8(bytes[..length].to_vec()).ok()?);
        }
        Some(args)
    }

    #[tokio::test]
    async fn redis_state_is_session_bound_and_consumed_once() {
        let (redis_url, server) = serve_redis_fixture().await;
        let client = AuthClient::new("http://127.0.0.1:1", reqwest::Client::new())
            .with_state_store(&redis_url)
            .unwrap();
        let state = client.create_oauth_state("session-a").await.unwrap();
        assert!(matches!(
            client.consume_oauth_state(&state, "session-b").await,
            Err(AuthClientError::InvalidState)
        ));
        assert!(
            client
                .consume_oauth_state(&state, "session-a")
                .await
                .is_ok()
        );
        assert!(matches!(
            client.consume_oauth_state(&state, "session-a").await,
            Err(AuthClientError::InvalidState)
        ));
        server.abort();
    }

    #[test]
    fn authorize_url_shape() {
        let c = AuthClient::new("https://auth.example/", reqwest::Client::new());
        assert_eq!(
            c.authorize_url("https://opencode.ai/auth/callback", "s1"),
            "https://auth.example/authorize?client_id=app&redirect_uri=https%3A%2F%2Fopencode.ai%2Fauth%2Fcallback&response_type=code&state=s1"
        );
    }

    #[test]
    fn session_cookie_roundtrip() {
        let codec = SessionCodec::new("fixture-session-secret-0123456789abcdef");
        let mut s = codec.load(None, 1_000);
        s.data.login("acc_1", "a@x");
        let cookie = codec.set_cookie(&s, 1_000).unwrap();
        assert!(cookie.starts_with("auth=Fe26.2**"));
        assert!(cookie.ends_with("; Path=/; Expires=Fri, 01 Jan 1971 00:00:01 GMT; HttpOnly"));
        let sealed = cookie
            .trim_start_matches("auth=")
            .split(';')
            .next()
            .unwrap();
        let back = codec.load(Some(sealed), 2_000);
        assert_eq!(back, s);
        assert!(codec.load(Some("garbage"), 2_000).data.account.is_none());
        assert!(!format!("{codec:?}").contains("fixture"));
    }
}
