//! Purpose: verify workspace invitation persistence and delivery against fixture MySQL with local mail capture.
//! Scope: integration test using baseline migrations and in-memory mail only.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/membership.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! |-- opencode_hosted_adapters::console::db::TestDatabase
//! |-- opencode_hosted_adapters::console::mail::CaptureInviteMailer
//! |-- opencode_hosted_domain::console::{common::Actor, identity::AuthProvider, membership::Invite}
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: all messages are captured locally; no production provider or external mail service is called.
//! Contract tests: this file

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::mail::{
    CaptureInviteMailer, InviteMail, InviteMailer, SesInviteMailConfig, SesInviteMailer,
};
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::{Actor, Role};
use opencode_hosted_domain::console::identity::AuthProvider;
use opencode_hosted_domain::console::membership::Invite;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

async fn account(store: &TenantStore, email: &str) -> String {
    let id = store.create_account(None).await.unwrap();
    store
        .upsert_auth_links(&id, AuthProvider::Github, &format!("gh-{email}"), email)
        .await
        .unwrap();
    id
}

async fn admin(store: &TenantStore, account_id: &str, email: &str) -> Actor {
    let workspace_id = store
        .create_workspace(
            &Actor::Account {
                account_id: account_id.into(),
                email: email.into(),
            },
            "Invite Fixture",
        )
        .await
        .unwrap();
    let member = store
        .admission_member(&workspace_id, &[account_id.to_string()])
        .await
        .unwrap()
        .unwrap();
    Actor::User {
        user_id: member.user_id,
        workspace_id,
        account_id: account_id.into(),
        role: Role::Admin,
    }
}

type CapturedRequestParts = (axum::http::Method, axum::http::Uri, HeaderMap, Bytes);

#[derive(Clone, Default)]
struct CapturedRequest(Arc<Mutex<Option<CapturedRequestParts>>>);

async fn receive(
    State(capture): State<CapturedRequest>,
    request: axum::http::Request<axum::body::Body>,
) -> StatusCode {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    *capture.0.lock().await = Some((parts.method, parts.uri, parts.headers, body));
    StatusCode::OK
}

#[tokio::test]
async fn ses_v2_capture_checks_provider_payload_and_sigv4_headers() {
    let capture = CapturedRequest::default();
    let app = Router::new()
        .route("/v2/email/outbound-emails", post(receive))
        .with_state(capture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/v2/email/outbound-emails",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = SesInviteMailConfig::new(
        "fixture-key",
        "fixture-secret",
        "us-east-1",
        "OpenCode Zen <contact@anoma.ly>",
    )
    .with_local_endpoint(endpoint)
    .unwrap();
    let mailer = SesInviteMailer::new(config).unwrap();
    mailer
        .send_invite(&InviteMail {
            to: "new@example.test".into(),
            inviter: "owner@example.test".into(),
            workspace_id: "wrk_fixture".into(),
            workspace_name: "Acme".into(),
        })
        .await
        .unwrap();
    server.abort();

    let (method, uri, headers, body) = capture.0.lock().await.take().unwrap();
    assert_eq!(method, "POST");
    assert_eq!(uri.path(), "/v2/email/outbound-emails");
    assert_eq!(headers["x-amz-target"], "SES.SendEmail");
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256 Credential=fixture-key/")
    );
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["FromEmailAddress"], "OpenCode Zen <contact@anoma.ly>");
    assert_eq!(body["Destination"]["ToAddresses"][0], "new@example.test");
    assert_eq!(
        body["Content"]["Simple"]["Subject"]["Data"],
        "You've been invited to join the Acme workspace on OpenCode"
    );
    assert!(
        body["Content"]["Simple"]["Body"]["Html"]["Data"]
            .as_str()
            .unwrap()
            .contains("https://opencode.ai/workspace/wrk_fixture")
    );
    assert!(
        body["Content"]["Simple"]["Body"]["Text"]["Data"]
            .as_str()
            .unwrap()
            .contains("owner@example.test")
    );
    assert!(!format!("{mailer:?}").contains("fixture-secret"));
}

#[test]
fn ses_test_endpoint_rejects_non_loopback_hosts() {
    let config = SesInviteMailConfig::new(
        "fixture-key",
        "fixture-secret",
        "us-east-1",
        "OpenCode Zen <contact@anoma.ly>",
    );
    assert!(
        config
            .with_local_endpoint(Url::parse("https://email.us-east-1.amazonaws.com/").unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn invite_persists_duplicate_and_existing_account_invites_and_swallows_mail_failure() {
    let database = TestDatabase::create("invite_mail").await.unwrap();
    let store = TenantStore::new(database.db.clone());
    let owner_id = account(&store, "owner@example.test").await;
    let owner = admin(&store, &owner_id, "owner@example.test").await;
    let workspace_id = owner.workspace().unwrap().to_owned();
    let capture = CaptureInviteMailer::new();

    let pending = Invite {
        email: "pending@example.test".into(),
        role: Role::Member,
        monthly_limit: Some(15),
    };
    store
        .invite_member(&owner, &pending, &capture)
        .await
        .unwrap();
    let first_mail = capture.sent().into_iter().next().unwrap();
    assert_eq!(first_mail.to, "pending@example.test");
    assert_eq!(first_mail.inviter, "owner@example.test");
    assert_eq!(first_mail.workspace_id, workspace_id);
    assert_eq!(first_mail.workspace_name, "Invite Fixture");

    let mut conn = database.db.conn().await.unwrap();
    let original_id: String = conn
        .exec_first(
            "SELECT `id` FROM `user` WHERE `workspace_id` = ? AND `email` = ?",
            (&workspace_id, "pending@example.test"),
        )
        .await
        .unwrap()
        .unwrap();
    conn.exec_drop(
        "UPDATE `user` SET `time_deleted` = now() WHERE `workspace_id` = ? AND `id` = ?",
        (&workspace_id, &original_id),
    )
    .await
    .unwrap();
    drop(conn);

    store
        .invite_member(
            &owner,
            &Invite {
                email: "pending@example.test".into(),
                role: Role::Admin,
                monthly_limit: Some(25),
            },
            &capture,
        )
        .await
        .unwrap();
    let mut conn = database.db.conn().await.unwrap();
    let duplicate: (String, String, Option<i32>, bool) = conn
        .exec_first(
            "SELECT `id`, `role`, `monthly_limit`, `time_deleted` IS NULL FROM `user` WHERE `workspace_id` = ? AND `email` = ?",
            (&workspace_id, "pending@example.test"),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(duplicate, (original_id, "admin".into(), Some(25), true));
    drop(conn);

    let existing_id = account(&store, "existing@example.test").await;
    let existing = Invite {
        email: "existing@example.test".into(),
        role: Role::Member,
        monthly_limit: None,
    };
    store
        .invite_member(&owner, &existing, &capture)
        .await
        .unwrap();
    store
        .invite_member(&owner, &existing, &capture)
        .await
        .unwrap();
    let mut conn = database.db.conn().await.unwrap();
    let member_id: String = conn
        .exec_first(
            "SELECT `id` FROM `user` WHERE `workspace_id` = ? AND `account_id` = ?",
            (&workspace_id, &existing_id),
        )
        .await
        .unwrap()
        .unwrap();
    let key_count: i64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM `key` WHERE `workspace_id` = ? AND `user_id` = ?",
            (&workspace_id, &member_id),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        key_count, 1,
        "duplicate existing-account invites keep one default key"
    );
    drop(conn);

    let member_actor = Actor::User {
        user_id: match &owner {
            Actor::User { user_id, .. } => user_id.clone(),
            _ => unreachable!(),
        },
        workspace_id: workspace_id.clone(),
        account_id: owner_id.clone(),
        role: Role::Member,
    };
    let sent_before_denial = capture.sent().len();
    assert!(
        store
            .invite_member(&member_actor, &pending, &capture)
            .await
            .is_err()
    );
    assert_eq!(capture.sent().len(), sent_before_denial);

    let failing_capture = CaptureInviteMailer::new();
    failing_capture.fail_with("fixture SES rejection");
    let failed_delivery = Invite {
        email: "persisted-despite-mail-failure@example.test".into(),
        role: Role::Member,
        monthly_limit: None,
    };
    store
        .invite_member(&owner, &failed_delivery, &failing_capture)
        .await
        .unwrap();
    assert_eq!(failing_capture.sent().len(), 1);
    let mut conn = database.db.conn().await.unwrap();
    let persisted: Option<String> = conn
        .exec_first(
            "SELECT `id` FROM `user` WHERE `workspace_id` = ? AND `email` = ? AND `time_deleted` IS NULL",
            (&workspace_id, &failed_delivery.email),
        )
        .await
        .unwrap();
    assert!(
        persisted.is_some(),
        "mail failure must not roll back the invitation"
    );
    drop(conn);

    database.drop_database().await.unwrap();
}
