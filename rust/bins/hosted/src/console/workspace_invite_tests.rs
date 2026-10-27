//! Purpose: accept workspace member invite HTTP behavior through Axum and fixture MySQL.
//! Scope: same-origin, admitted actor, admin authorization, invite persistence and captured delivery.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/console/workspace.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- axum::{body::Body, http::Request, Router}
//! |-- mysql_async::prelude::Queryable
//! |-- opencode_hosted_adapters::console::auth::SessionCodec
//! |-- opencode_hosted_adapters::console::db::TestDatabase
//! |-- serde_json::{Value, json}
//! `-- tower::ServiceExt
//! Used by:
//! `-- hosted/src/console/workspace.rs
//! Invariants: all mail is captured in memory; requests use fixture MySQL and synthetic sealed sessions.
//! Contract tests: this file

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::auth::SessionCodec;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::mail::CaptureInviteMailer;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use opencode_hosted_domain::console::identity::AuthProvider;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

use super::*;
use crate::console::state::ConsoleConfig;

const SECRET: &str = "workspace-invite-test-secret-with-at-least-32-bytes";

fn cookie(account_id: &str, email: &str) -> String {
    let codec = SessionCodec::new(SECRET);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut session = codec.load(None, now);
    session.data.login(account_id, email);
    codec.set_cookie(&session, now).unwrap()
}

async fn app(
    db: opencode_hosted_adapters::console::db::ConsoleDb,
    mailer: CaptureInviteMailer,
) -> Router {
    let config = ConsoleConfig {
        auth: auth::Config {
            issuer_url: None,
            session_secret: Some(SECRET.into()),
            state_redis_url: None,
            console_url: Some("https://console.opencode.ai".into()),
            public_origin: Some("https://console.opencode.ai".into()),
        },
        workspace: Config {
            invite_mailer: Some(Arc::new(mailer)),
            ..Default::default()
        },
        ..Default::default()
    };
    super::routes().with_state(ConsoleState {
        config: Arc::new(config),
        db,
    })
}

async fn request(
    router: Router,
    cookie: Option<&str>,
    origin: Option<&str>,
    workspace_id: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/workspaces/{workspace_id}/members"))
        .header("host", "console.opencode.ai")
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie.split(';').next().unwrap());
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = router
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn member_post_requires_admin_and_same_origin_then_persists_and_captures_invite() {
    let fixture = TestDatabase::create("workspace_invite_http").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let owner_id = store.create_account(None).await.unwrap();
    store
        .upsert_auth_links(
            &owner_id,
            AuthProvider::Github,
            "github-workspace-invite-owner",
            "owner@example.test",
        )
        .await
        .unwrap();
    let workspace_id = store
        .create_workspace(
            &Actor::Account {
                account_id: owner_id.clone(),
                email: "owner@example.test".into(),
            },
            "Route Invite",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `billing` SET `subscription_id` = 'sub_workspace_invite_fixture' WHERE `workspace_id` = ?",
        (&workspace_id,),
    )
    .await
    .unwrap();
    let member_id = "acc_workspace_invite_member";
    conn.exec_drop("INSERT INTO `account` (`id`) VALUES (?)", (member_id,))
        .await
        .unwrap();
    conn.exec_drop(
        "INSERT INTO `user` (`id`, `name`, `account_id`, `workspace_id`, `role`) VALUES ('usr_workspace_invite_member', '', ?, ?, 'member')",
        (member_id, &workspace_id),
    )
    .await
    .unwrap();
    drop(conn);

    let mailer = CaptureInviteMailer::new();
    let router = app(fixture.db.clone(), mailer.clone()).await;
    let owner_cookie = cookie(&owner_id, "owner@example.test");
    let payload = json!({
        "email": "  new-member@example.test  ",
        "role": "member",
        "monthlyLimit": 42
    });
    let (status, _) = request(
        router.clone(),
        Some(&owner_cookie),
        Some("https://evil.example"),
        &workspace_id,
        payload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(mailer.sent().is_empty());

    let member_cookie = cookie(member_id, "member@example.test");
    let (status, _) = request(
        router.clone(),
        Some(&member_cookie),
        Some("https://console.opencode.ai"),
        &workspace_id,
        payload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(mailer.sent().is_empty());

    let (status, _) = request(
        router,
        Some(&owner_cookie),
        Some("https://console.opencode.ai"),
        &workspace_id,
        payload,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let sent = mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "new-member@example.test");
    assert_eq!(sent[0].inviter, "owner@example.test");
    assert_eq!(sent[0].workspace_id, workspace_id);
    assert_eq!(sent[0].workspace_name, "Route Invite");

    let members = store
        .list_members(&Actor::User {
            user_id: "usr_workspace_invite_member".into(),
            workspace_id: workspace_id.clone(),
            account_id: member_id.into(),
            role: opencode_hosted_domain::console::common::Role::Member,
        })
        .await
        .unwrap();
    let invite = members
        .iter()
        .find(|member| member.email.as_deref() == Some("new-member@example.test"))
        .unwrap();
    assert_eq!(invite.monthly_limit, Some(42));

    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn member_post_rejects_missing_session_and_invalid_input() {
    let fixture = TestDatabase::create("workspace_invite_denied")
        .await
        .unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let owner_id = store.create_account(None).await.unwrap();
    store
        .upsert_auth_links(
            &owner_id,
            AuthProvider::Github,
            "github-workspace-invite-denied",
            "denied@example.test",
        )
        .await
        .unwrap();
    let workspace_id = store
        .create_workspace(
            &Actor::Account {
                account_id: owner_id.clone(),
                email: "denied@example.test".into(),
            },
            "Denied Invite",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `billing` SET `subscription_id` = 'sub_workspace_invite_denied' WHERE `workspace_id` = ?",
        (&workspace_id,),
    )
    .await
    .unwrap();
    drop(conn);
    let mailer = CaptureInviteMailer::new();
    let router = app(fixture.db.clone(), mailer.clone()).await;
    let (status, _) = request(
        router.clone(),
        None,
        Some("https://console.opencode.ai"),
        &workspace_id,
        json!({ "email": "nobody@example.test", "role": "member" }),
    )
    .await;
    assert_ne!(status, StatusCode::NO_CONTENT);
    let owner_cookie = cookie(&owner_id, "denied@example.test");
    for invalid in [
        json!({ "email": "  ", "role": "member" }),
        json!({ "email": "bad-limit@example.test", "role": "member", "monthlyLimit": -1 }),
    ] {
        let (status, _) = request(
            router.clone(),
            Some(&owner_cookie),
            Some("https://console.opencode.ai"),
            &workspace_id,
            invalid,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert!(mailer.sent().is_empty());
    fixture.drop_database().await.unwrap();
}
