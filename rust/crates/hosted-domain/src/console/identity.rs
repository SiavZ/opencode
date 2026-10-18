//! Purpose: console identity rules: OAuth redirect allowlist, verified-email identity resolution
//! from GitHub/Google, non-production email restriction, identity-link selection, console
//! actor admission and migrated-console redirects. Pure rules, no I/O.
//! Scope: per-request decisions over values loaded by adapters.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/console/common.rs
//! |-- hosted-domain/src/console/identity/session.rs
//! `-- hosted-domain/src/console/identity/tests.rs
//! External:
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: allowlist equals `packages/console/function/src/auth-redirect.ts:1-18`; provider
//! match wins over email match (`function/src/auth.ts:174-176`); migrated workspaces redirect
//! with 302 for GET/HEAD and 303 otherwise, always `Cache-Control: no-store`
//! (`app/src/context/auth.ts:106-114`).
//! Contract tests: hosted-domain/src/console/identity/tests.rs

use serde::{Deserialize, Serialize};

use super::common::{Actor, ConsoleError, Role};

mod session;

pub use session::{
    AccountSelection, AuthSession, OrderedAccounts, SessionAccount, select_session_account,
};

/// `isAllowedAuthorizationRedirect(clientID, redirectURI)`.
pub fn is_allowed_authorization_redirect(client_id: &str, redirect_uri: &str) -> bool {
    if client_id != "app" {
        return false;
    }
    let Ok(redirect) = url::Url::parse(redirect_uri) else {
        return false;
    };
    let host = redirect.host_str().unwrap_or("");
    let scheme = redirect.scheme();
    if host == "localhost" || host == "127.0.0.1" {
        return scheme == "http" || scheme == "https";
    }
    scheme == "https" && (host == "opencode.ai" || host.ends_with(".opencode.ai"))
}

/// Identity providers enabled at the issuer (`function/src/auth.ts:56-68`). The email code
/// provider is commented out in the baseline and is not a runtime path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthProvider {
    Email,
    Github,
    Google,
}

impl AuthProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthProvider::Email => "email",
            AuthProvider::Github => "github",
            AuthProvider::Google => "google",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "email" => Some(AuthProvider::Email),
            "github" => Some(AuthProvider::Github),
            "google" => Some(AuthProvider::Google),
            _ => None,
        }
    }
}

/// One entry of GitHub `GET /user/emails`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubEmail {
    pub email: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub verified: bool,
}

/// Provider response normalized for resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderIdentity {
    /// GitHub: numeric `user.id` and the `/user/emails` list.
    Github {
        user_id: serde_json::Value,
        emails: Vec<GithubEmail>,
    },
    /// Google OIDC id token claims.
    Google {
        sub: Option<String>,
        email: Option<String>,
        email_verified: bool,
    },
}

/// Verified identity: provider subject plus verified email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub provider: AuthProvider,
    pub subject: String,
    pub email: String,
}

/// Resolves subject and verified email exactly like `function/src/auth.ts:134-170`.
/// `stage` is `Resource.App.stage`.
pub fn resolve_identity(
    identity: &ProviderIdentity,
    stage: &str,
) -> Result<VerifiedIdentity, ConsoleError> {
    let invalid = |m: &str| ConsoleError::Validation(m.to_string());
    let (provider, subject, email) = match identity {
        ProviderIdentity::Github { user_id, emails } => {
            // `user.id.toString()`: numbers print without quotes, strings verbatim.
            let subject = match user_id {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            };
            let primary = emails
                .iter()
                .find(|e| e.primary)
                .ok_or_else(|| invalid("No primary email found for GitHub user"))?;
            if !primary.verified {
                return Err(invalid("Primary email for GitHub user not verified"));
            }
            (AuthProvider::Github, subject, Some(primary.email.clone()))
        }
        ProviderIdentity::Google {
            sub,
            email,
            email_verified,
        } => {
            if !email_verified {
                return Err(invalid("Google email not verified"));
            }
            (AuthProvider::Google, sub.clone(), email.clone())
        }
    };
    let email = email
        .filter(|e| !e.is_empty())
        .ok_or_else(|| invalid("No email found"))?;
    let subject = subject
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("No subject found"))?;
    if stage != "production" && !email.ends_with("@anoma.ly") {
        return Err(invalid("Invalid email"));
    }
    Ok(VerifiedIdentity {
        provider,
        subject,
        email,
    })
}

/// Existing auth row matched by provider subject or by email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMatch {
    pub provider: AuthProvider,
    pub account_id: String,
}

/// Provider match wins over the email match (`function/src/auth.ts:174-176`).
pub fn select_account(provider: AuthProvider, matches: &[AuthMatch]) -> Option<&str> {
    matches
        .iter()
        .find(|m| m.provider == provider)
        .or_else(|| matches.iter().find(|m| m.provider == AuthProvider::Email))
        .map(|m| m.account_id.as_str())
}

/// Issuer success outcome after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuerOutcome {
    /// Missing identity or no subscribed Black workspace: 302 to `<host without auth.>/console`.
    RedirectToNewConsole,
    /// Upsert provider + email auth rows and issue the `account` subject.
    IssueAccount { account_id: String, email: String },
}

/// `redirectToNewConsole(request)` in `function/src/auth.ts:211-215`.
pub fn issuer_new_console_url(request_url: &str) -> Result<String, ConsoleError> {
    let mut url = url::Url::parse(request_url)
        .map_err(|e| ConsoleError::Validation(format!("Invalid URL: {e}")))?;
    url.set_path("/console");
    url.set_query(None);
    url.set_fragment(None);
    if let Some(host) = url.host_str().map(str::to_string)
        && let Some(stripped) = host.strip_prefix("auth.")
    {
        let _ = url.set_host(Some(stripped));
    }
    Ok(url.to_string())
}

/// Membership row loaded for workspace actor admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub user_id: String,
    pub workspace_id: String,
    pub account_id: Option<String>,
    pub role: Role,
    pub migrated: bool,
}

/// HTTP redirect decision produced by actor admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub location: String,
    pub status: u16,
    pub no_store: bool,
}

/// Admission result for workspace-scoped requests (`context/auth.ts:64-131`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Actor(Actor),
    Redirect(Redirect),
}

/// `redirectToNewConsole()` in the app (`context/auth.ts:159-163`).
pub fn new_console_redirect(console_url: Option<&str>) -> Result<Redirect, ConsoleError> {
    let destination = console_url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ConsoleError::Validation("New Console URL is not configured".into()))?;
    Ok(Redirect {
        location: format!("{destination}/login"),
        status: 302,
        no_store: true,
    })
}

/// Decides admission once the adapter has loaded the Black accounts and the member row.
/// `black_accounts` empty -> new-console redirect; member missing -> `/auth/authorize` (302).
pub fn admit_workspace(
    has_session_accounts: bool,
    black_accounts: &[String],
    member: Option<&WorkspaceMember>,
    method: &str,
    console_url: Option<&str>,
) -> Result<Admission, ConsoleError> {
    if has_session_accounts {
        if black_accounts.is_empty() {
            return new_console_redirect(console_url).map(Admission::Redirect);
        }
        if let Some(member) = member {
            if member.migrated {
                let destination = console_url.filter(|u| !u.is_empty()).ok_or_else(|| {
                    ConsoleError::Validation("New Console URL is not configured".into())
                })?;
                let status = if method == "GET" || method == "HEAD" {
                    302
                } else {
                    303
                };
                return Ok(Admission::Redirect(Redirect {
                    location: format!("{destination}/login"),
                    status,
                    no_store: true,
                }));
            }
            return Ok(Admission::Actor(Actor::User {
                user_id: member.user_id.clone(),
                workspace_id: member.workspace_id.clone(),
                account_id: member.account_id.clone().unwrap_or_default(),
                role: member.role,
            }));
        }
    }
    Ok(Admission::Redirect(Redirect {
        location: "/auth/authorize".into(),
        status: 302,
        no_store: false,
    }))
}

/// `/auth/[...callback]` next path: `/auth/callback` -> `/auth`, `/auth/callback/x` -> `/x`.
pub fn callback_next_path(pathname: &str) -> String {
    if pathname == "/auth/callback" {
        return "/auth".into();
    }
    pathname.replacen("/auth/callback", "", 1)
}

#[cfg(test)]
mod tests;
