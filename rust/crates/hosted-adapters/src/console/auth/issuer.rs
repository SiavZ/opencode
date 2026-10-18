//! Purpose: console OpenAuth issuer success flow: fetch GitHub identity (user + emails) or take
//! Google OIDC claims, resolve the verified identity, find the account by provider/email link,
//! claim invitations, require a subscribed Black workspace, upsert links and return the
//! `account` subject properties, or redirect to the new Console.
//! Scope: per-callback adapter; GitHub HTTP via a configurable API base (fixture-replaceable).
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! `-- reqwest::Client
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! `-- hosted-adapters/tests/console_identity_auth.rs
//! Invariants: order follows `packages/console/function/src/auth.ts:128-206` exactly: identity
//! checks, stage check, link lookup, invitation claim under the account actor, Black check,
//! link upsert. The OpenAuth token/KV/provider-redirect machinery itself is NOT ported here
//! (blocker `p12.identity-issuer-runtime` in children/identity.md).
//! Contract tests: hosted-adapters/tests/console_identity_auth.rs

use opencode_hosted_domain::console::identity::{
    AuthProvider, GithubEmail, IssuerOutcome, ProviderIdentity, is_allowed_authorization_redirect,
    resolve_identity, select_account,
};

use crate::console::tenant::{TenantResult, TenantStore};

/// Issuer `/authorize` pre-check (`function/src/auth.ts:44-52`): `Some(400 body)` when rejected.
pub fn authorize_precheck(
    client_id: Option<&str>,
    redirect_uri: Option<&str>,
) -> Option<&'static str> {
    let redirect = redirect_uri?;
    if is_allowed_authorization_redirect(client_id.unwrap_or(""), redirect) {
        return None;
    }
    Some("Unauthorized client")
}

/// Fetches GitHub `/user/emails` and `/user` with the provider access token.
pub async fn github_identity(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
) -> Result<ProviderIdentity, reqwest::Error> {
    let get = |path: &str| {
        http.get(format!("{}{path}", api_base.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("User-Agent", "opencode")
            .header("Accept", "application/vnd.github+json")
            .send()
    };
    let emails: Vec<GithubEmail> = get("/user/emails").await?.json().await?;
    let user: serde_json::Value = get("/user").await?.json().await?;
    Ok(ProviderIdentity::Github {
        user_id: user.get("id").cloned().unwrap_or(serde_json::Value::Null),
        emails,
    })
}

/// Issuer failure that the OpenAuth issuer would surface as an error response.
#[derive(Debug, thiserror::Error)]
pub enum IssuerError {
    #[error(transparent)]
    Identity(#[from] opencode_hosted_domain::console::common::ConsoleError),
    #[error(transparent)]
    Tenant(#[from] crate::console::tenant::TenantError),
}

/// `success(ctx, response)` after the provider identity is known.
pub async fn issuer_success(
    store: &TenantStore,
    identity: &ProviderIdentity,
    stage: &str,
) -> Result<IssuerOutcome, IssuerError> {
    let verified = resolve_identity(identity, stage)?;
    let matches = store
        .auth_matches(verified.provider, &verified.subject, &verified.email)
        .await?;
    let Some(account_id) = select_account(verified.provider, &matches).map(str::to_string) else {
        return Ok(IssuerOutcome::RedirectToNewConsole);
    };
    if !claim_and_check_black(store, &account_id, &verified.email).await? {
        return Ok(IssuerOutcome::RedirectToNewConsole);
    }
    store
        .upsert_auth_links(
            &account_id,
            verified.provider,
            &verified.subject,
            &verified.email,
        )
        .await?;
    Ok(IssuerOutcome::IssueAccount {
        account_id,
        email: verified.email,
    })
}

async fn claim_and_check_black(
    store: &TenantStore,
    account_id: &str,
    email: &str,
) -> TenantResult<bool> {
    store.join_invited_workspaces(account_id, email).await?;
    store.has_black_workspace(account_id).await
}

/// `subjects.account` properties issued on success: `{accountID, email, newAccount: false}`.
pub fn account_subject_properties(account_id: &str, email: &str) -> serde_json::Value {
    serde_json::json!({ "accountID": account_id, "email": email, "newAccount": false })
}

/// Provider name helper for OpenAuth `response.provider`.
pub fn provider_name(provider: AuthProvider) -> &'static str {
    provider.as_str()
}
