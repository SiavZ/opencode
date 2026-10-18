//! Purpose: unit tests for console identity rules (allowlist cases from
//! `packages/console/function/src/auth-redirect.test.ts`, resolution, admission, redirects).
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- hosted-domain/src/console/identity.rs
//! Invariants: exercises the public functions only.
//! Contract tests: this file.
use super::*;

#[test]
fn allowlist_matches_baseline_tests() {
    // packages/console/function/src/auth-redirect.test.ts:5-17
    assert!(is_allowed_authorization_redirect(
        "app",
        "https://opencode.ai/auth/callback"
    ));
    assert!(is_allowed_authorization_redirect(
        "app",
        "https://dev.opencode.ai/auth/callback"
    ));
    assert!(is_allowed_authorization_redirect(
        "app",
        "http://localhost:3000/auth/callback"
    ));
    assert!(is_allowed_authorization_redirect(
        "app",
        "http://127.0.0.1:3000/auth/callback"
    ));
    assert!(!is_allowed_authorization_redirect(
        "other",
        "https://opencode.ai/auth/callback"
    ));
    assert!(!is_allowed_authorization_redirect(
        "app",
        "https://evil.example/callback"
    ));
    assert!(!is_allowed_authorization_redirect(
        "app",
        "https://opencode.ai.evil.example/callback"
    ));
    assert!(!is_allowed_authorization_redirect(
        "app",
        "javascript:alert(1)"
    ));
    assert!(!is_allowed_authorization_redirect(
        "app",
        "http://opencode.ai/x"
    ));
    assert!(!is_allowed_authorization_redirect("app", "not a url"));
}

#[test]
fn github_resolution_rules() {
    let emails = vec![
        GithubEmail {
            email: "a@anoma.ly".into(),
            primary: false,
            verified: true,
        },
        GithubEmail {
            email: "b@anoma.ly".into(),
            primary: true,
            verified: true,
        },
    ];
    let id = resolve_identity(
        &ProviderIdentity::Github {
            user_id: serde_json::json!(42),
            emails: emails.clone(),
        },
        "dev",
    )
    .unwrap();
    assert_eq!(id.subject, "42");
    assert_eq!(id.email, "b@anoma.ly");
    let unverified = vec![GithubEmail {
        email: "x@y.z".into(),
        primary: true,
        verified: false,
    }];
    assert_eq!(
        resolve_identity(
            &ProviderIdentity::Github {
                user_id: serde_json::json!(1),
                emails: unverified
            },
            "production"
        )
        .unwrap_err()
        .to_string(),
        "Primary email for GitHub user not verified"
    );
    assert_eq!(
        resolve_identity(
            &ProviderIdentity::Github {
                user_id: serde_json::json!(1),
                emails: vec![]
            },
            "production"
        )
        .unwrap_err()
        .to_string(),
        "No primary email found for GitHub user"
    );
}

#[test]
fn google_and_stage_rules() {
    let google = |verified| ProviderIdentity::Google {
        sub: Some("s".into()),
        email: Some("u@example.com".into()),
        email_verified: verified,
    };
    assert_eq!(
        resolve_identity(&google(false), "production")
            .unwrap_err()
            .to_string(),
        "Google email not verified"
    );
    assert!(resolve_identity(&google(true), "production").is_ok());
    assert_eq!(
        resolve_identity(&google(true), "dev")
            .unwrap_err()
            .to_string(),
        "Invalid email"
    );
}

#[test]
fn provider_match_wins_over_email_match() {
    let matches = vec![
        AuthMatch {
            provider: AuthProvider::Email,
            account_id: "acc_email".into(),
        },
        AuthMatch {
            provider: AuthProvider::Github,
            account_id: "acc_gh".into(),
        },
    ];
    assert_eq!(
        select_account(AuthProvider::Github, &matches),
        Some("acc_gh")
    );
    assert_eq!(
        select_account(AuthProvider::Google, &matches),
        Some("acc_email")
    );
    assert_eq!(select_account(AuthProvider::Google, &[]), None);
}

#[test]
fn issuer_redirect_strips_auth_subdomain() {
    assert_eq!(
        issuer_new_console_url("https://auth.opencode.ai/github/callback?code=1").unwrap(),
        "https://opencode.ai/console"
    );
    assert_eq!(
        issuer_new_console_url("http://localhost:3000/x").unwrap(),
        "http://localhost:3000/console"
    );
}

#[test]
fn admission_redirects() {
    let member = WorkspaceMember {
        user_id: "usr_1".into(),
        workspace_id: "wrk_1".into(),
        account_id: Some("acc_1".into()),
        role: Role::Admin,
        migrated: true,
    };
    let black = vec!["acc_1".to_string()];
    let url = Some("https://new.example");
    let get = admit_workspace(true, &black, Some(&member), "GET", url).unwrap();
    assert_eq!(
        get,
        Admission::Redirect(Redirect {
            location: "https://new.example/login".into(),
            status: 302,
            no_store: true
        })
    );
    let post = admit_workspace(true, &black, Some(&member), "POST", url).unwrap();
    assert!(matches!(
        post,
        Admission::Redirect(Redirect { status: 303, .. })
    ));
    assert!(matches!(
        admit_workspace(true, &[], Some(&member), "GET", url).unwrap(),
        Admission::Redirect(Redirect {
            status: 302,
            no_store: true,
            ..
        })
    ));
    assert!(matches!(
        admit_workspace(false, &[], None, "GET", url).unwrap(),
        Admission::Redirect(Redirect { ref location, .. }) if location == "/auth/authorize"
    ));
    assert_eq!(
        admit_workspace(true, &[], None, "GET", None)
            .unwrap_err()
            .to_string(),
        "New Console URL is not configured"
    );
    let live = WorkspaceMember {
        migrated: false,
        ..member
    };
    assert!(matches!(
        admit_workspace(true, &black, Some(&live), "POST", url).unwrap(),
        Admission::Actor(Actor::User {
            role: Role::Admin,
            ..
        })
    ));
}

#[test]
fn callback_paths() {
    assert_eq!(callback_next_path("/auth/callback"), "/auth");
    assert_eq!(
        callback_next_path("/auth/callback/workspace/wrk_1"),
        "/workspace/wrk_1"
    );
}
