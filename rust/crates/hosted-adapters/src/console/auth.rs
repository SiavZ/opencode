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
//! Invariants: client id is `app`; authorize uses `response_type=code` with a random UUID state
//! and no PKCE (console `AuthClient.authorize(url, "code")`); exchange posts the urlencoded form
//! with an empty `code_verifier`; decode does not verify the JWT signature, matching
//! `@openauthjs/openauth` client `decode` (recorded as a security note); cookie name `auth`,
//! maxAge one year, `HttpOnly`, not `Secure` (`app/src/context/auth.ts:30-40`).
//! Contract tests: hosted-adapters/src/console/auth/iron.rs, hosted-adapters/tests/console_identity_auth.rs

pub mod iron;
pub mod issuer;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use opencode_hosted_domain::console::common::{Actor, ConsoleError};
use opencode_hosted_domain::console::identity::{
    AccountSelection, Admission, AuthSession, admit_workspace, new_console_redirect,
    select_session_account,
};

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
    #[error("issuer transport: {0}")]
    Transport(String),
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
}

impl AuthClient {
    pub fn new(issuer: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            issuer: issuer.into().trim_end_matches('/').to_string(),
            http,
        }
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
            .send()
            .await
            .map_err(|e| AuthClientError::Transport(e.to_string()))?;
        let ok = res.status().is_success();
        let json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| AuthClientError::Transport(e.to_string()))?;
        if !ok {
            return Err(AuthClientError::InvalidAuthorizationCode);
        }
        Ok(json
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string())
    }
}

/// `AuthClient.decode(token)`: unverified JWT payload; requires string `type` and `sub`.
pub fn decode_account_subject(token: &str) -> Result<AccountSubject, AuthClientError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or(AuthClientError::InvalidAccessToken)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| AuthClientError::InvalidAccessToken)?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| AuthClientError::InvalidAccessToken)?;
    if !json.get("type").is_some_and(|v| v.is_string())
        || !json.get("sub").is_some_and(|v| v.is_string())
    {
        return Err(AuthClientError::InvalidAccessToken);
    }
    let props = json.get("properties").cloned().unwrap_or_default();
    let field = |k: &str| {
        props
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(AccountSubject {
        account_id: field("accountID"),
        email: field("email"),
    })
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
    fn from(e: ConsoleError) -> Self {
        AuthClientError::Transport(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_unverified_subject() {
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"type":"account","sub":"account:acc_1","properties":{"accountID":"acc_1","email":"a@x"}}"#,
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(
            decode_account_subject(&token).unwrap(),
            AccountSubject {
                account_id: "acc_1".into(),
                email: "a@x".into()
            }
        );
        assert!(matches!(
            decode_account_subject("bad"),
            Err(AuthClientError::InvalidAccessToken)
        ));
        let no_type = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"sub":"x"}"#));
        assert!(decode_account_subject(&no_type).is_err());
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
