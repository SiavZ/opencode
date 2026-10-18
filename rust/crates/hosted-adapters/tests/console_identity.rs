//! Purpose: fixture tests for the console tenant repository against local Docker MySQL with the
//! baseline migrations: workspace create, invite/claim, key visibility, cross-tenant denial,
//! BYOK, member rules, block batch and account erasure.
//! Scope: integration test (fixture, local MySQL only; mail goes to an in-memory recorder).
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! |-- hosted-domain/src/console/identity.rs
//! `-- hosted-domain/src/console/membership.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches a non-local database or real mail.
//! Contract tests: this file.

use std::sync::Mutex;

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::mail::{InviteMail, InviteMailer};
use opencode_hosted_adapters::console::tenant::{TenantError, TenantStore};
use opencode_hosted_domain::console::common::{Actor, Role};
use opencode_hosted_domain::console::identity::AuthProvider;
use opencode_hosted_domain::console::membership::Invite;

#[derive(Debug, Default)]
struct RecordingMailer {
    sent: Mutex<Vec<InviteMail>>,
    fail: bool,
}

#[async_trait::async_trait]
impl InviteMailer for RecordingMailer {
    async fn send_invite(&self, mail: &InviteMail) -> Result<(), String> {
        self.sent.lock().unwrap().push(mail.clone());
        if self.fail {
            return Err("fixture mail failure".into());
        }
        Ok(())
    }
}

async fn account(store: &TenantStore, email: &str) -> String {
    let id = store.create_account(None).await.unwrap();
    store
        .upsert_auth_links(&id, AuthProvider::Github, &format!("gh-{email}"), email)
        .await
        .unwrap();
    id
}

async fn admin_of(store: &TenantStore, account_id: &str, email: &str) -> Actor {
    let ws = store
        .create_workspace(
            &Actor::Account {
                account_id: account_id.into(),
                email: email.into(),
            },
            "Acme",
        )
        .await
        .unwrap();
    let member = store
        .admission_member(&ws, &[account_id.to_string()])
        .await
        .unwrap()
        .unwrap();
    Actor::User {
        user_id: member.user_id,
        workspace_id: ws,
        account_id: account_id.into(),
        role: member.role,
    }
}

#[tokio::test]
async fn fixture_tenant_lifecycle() {
    let t = TestDatabase::create("identity").await.unwrap();
    let store = TenantStore::new(t.db.clone());

    let alice = account(&store, "alice@anoma.ly").await;
    let admin = admin_of(&store, &alice, "alice@anoma.ly").await;
    assert!(matches!(
        admin,
        Actor::User {
            role: Role::Admin,
            ..
        }
    ));

    // Workspace create gave the admin one default key with a visible secret.
    let keys = store.list_keys(&admin).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "Default API Key");
    assert!(keys[0].key.as_deref().unwrap().starts_with("sk-"));
    assert_eq!(keys[0].key.as_deref().unwrap().len(), 67);

    // Invite a pending email (no account) and an existing account.
    let bob = account(&store, "bob@anoma.ly").await;
    let mailer = RecordingMailer {
        fail: true,
        ..Default::default()
    };
    for (email, role) in [
        ("carol@anoma.ly", Role::Member),
        ("bob@anoma.ly", Role::Member),
    ] {
        store
            .invite_member(
                &admin,
                &Invite {
                    email: email.into(),
                    role,
                    monthly_limit: Some(10),
                },
                &mailer,
            )
            .await
            .unwrap();
    }
    // Mail failure is swallowed, both mails were attempted with workspace name and inviter.
    let sent = mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].inviter, "alice@anoma.ly");
    assert_eq!(sent[0].workspace_name, "Acme");

    let members = store.list_members(&admin).await.unwrap();
    assert_eq!(members.len(), 3);
    let carol = members
        .iter()
        .find(|m| m.email.as_deref() == Some("carol@anoma.ly"))
        .unwrap();
    assert_eq!(carol.account_id, None);
    let bob_row = members
        .iter()
        .find(|m| m.account_id.as_deref() == Some(&bob))
        .unwrap();
    assert_eq!(bob_row.auth_email.as_deref(), Some("bob@anoma.ly"));
    assert_eq!(bob_row.monthly_limit, Some(10));

    // Admin sees every key but only its own secret; bob (existing account) got a default key.
    let keys = store.list_keys(&admin).await.unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.iter().filter(|k| k.key.is_some()).count(), 1);

    // Carol logs in later and claims the pending invitation.
    let carol_acc = account(&store, "carol@anoma.ly").await;
    store
        .join_invited_workspaces(&carol_acc, "carol@anoma.ly")
        .await
        .unwrap();
    let ws = admin.workspace().unwrap().to_string();
    let carol_member = store
        .admission_member(&ws, std::slice::from_ref(&carol_acc))
        .await
        .unwrap()
        .unwrap();
    let carol_actor = Actor::User {
        user_id: carol_member.user_id.clone(),
        workspace_id: ws.clone(),
        account_id: carol_acc.clone(),
        role: carol_member.role,
    };
    let carol_keys = store.list_keys(&carol_actor).await.unwrap();
    assert_eq!(carol_keys.len(), 1);
    assert!(carol_keys[0].key.is_some());

    // Member cannot remove someone else's key; admin can.
    let admin_key = store.list_keys(&admin).await.unwrap();
    let alice_key = admin_key
        .iter()
        .find(|k| k.key.is_some())
        .unwrap()
        .id
        .clone();
    store.remove_key(&carol_actor, &alice_key).await.unwrap();
    assert!(
        store
            .list_keys(&admin)
            .await
            .unwrap()
            .iter()
            .any(|k| k.id == alice_key)
    );

    // Member rules: member cannot invite/save providers; admin cannot remove self.
    let err = store
        .save_provider(&carol_actor, "openai", "sk-secret")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Action not allowed. Ask your workspace admin to perform this action."
    );
    let self_id = match &admin {
        Actor::User { user_id, .. } => user_id.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        store
            .remove_member(&admin, &self_id)
            .await
            .unwrap_err()
            .to_string(),
        "Expected not self actor, got self actor"
    );

    // Cross-tenant denial: a second workspace admin cannot touch the first workspace's rows.
    let mallory = account(&store, "mallory@anoma.ly").await;
    let other = admin_of(&store, &mallory, "mallory@anoma.ly").await;
    store
        .remove_member(&other, &carol_member.user_id)
        .await
        .unwrap();
    store.remove_key(&other, &alice_key).await.unwrap();
    assert_eq!(store.list_members(&admin).await.unwrap().len(), 3);
    assert!(
        store
            .list_keys(&admin)
            .await
            .unwrap()
            .iter()
            .any(|k| k.id == alice_key)
    );
    assert!(
        store
            .list_keys(&other)
            .await
            .unwrap()
            .iter()
            .all(|k| k.id != alice_key)
    );

    // BYOK upsert, list (scoped), remove.
    store
        .save_provider(&admin, "openai", "sk-one")
        .await
        .unwrap();
    store
        .save_provider(&admin, "openai", "sk-two")
        .await
        .unwrap();
    let providers = store.list_providers(&admin).await.unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].credentials, "sk-two");
    assert!(!format!("{:?}", providers[0]).contains("sk-two"));
    assert!(store.list_providers(&other).await.unwrap().is_empty());
    store.remove_provider(&admin, "openai").await.unwrap();
    assert!(store.list_providers(&admin).await.unwrap().is_empty());

    // Block batch dedupes and reports unknown ids.
    let batch = store
        .set_blocked_batch(&[ws.clone(), ws.clone(), "wrk_missing".into()], true)
        .await
        .unwrap();
    assert_eq!(batch.applied, vec![ws.clone()]);
    assert_eq!(batch.not_found, vec!["wrk_missing".to_string()]);
    assert!(matches!(
        store.unblock_workspace("wrk_missing").await,
        Err(TenantError::Console(_))
    ));

    // Erasure: keys revoked, memberships anonymized, auth removed, account soft-deleted.
    {
        let mut conn = t.db.conn().await.unwrap();
        conn.exec_drop(
            "INSERT INTO `coupon` (`email`, `type`) VALUES ('bob@anoma.ly', 'BUILDATHON')",
            (),
        )
        .await
        .unwrap();
    }
    store.erase_account("bob@anoma.ly").await.unwrap();
    let mut conn = t.db.conn().await.unwrap();
    let live_keys: i64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM `key` WHERE `user_id` = ? AND `time_deleted` IS NULL",
            (&bob_row.id,),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live_keys, 0);
    let user: (Option<String>, Option<String>, String, bool) = conn
        .exec_first(
            "SELECT `account_id`, `email`, `name`, `time_deleted` IS NOT NULL FROM `user` WHERE `id` = ?",
            (&bob_row.id,),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user, (None, None, String::new(), true));
    let auth: i64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM `auth` WHERE `account_id` = ?",
            (&bob,),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(auth, 0);
    let coupons: i64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM `coupon` WHERE `email` = 'bob@anoma.ly'",
            (),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(coupons, 0);
    assert!(
        store
            .account(&bob)
            .await
            .unwrap()
            .unwrap()
            .time_deleted
            .is_some()
    );
    assert_eq!(
        store
            .erase_account("bob@anoma.ly")
            .await
            .unwrap_err()
            .to_string(),
        "Account not found"
    );
    drop(conn);
    t.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_admission_queries() {
    let t = TestDatabase::create("admission").await.unwrap();
    let store = TenantStore::new(t.db.clone());
    let acc = account(&store, "dana@anoma.ly").await;
    let admin = admin_of(&store, &acc, "dana@anoma.ly").await;
    let ws = admin.workspace().unwrap().to_string();

    // No Black subscription yet: no black accounts, issuer check false.
    assert!(
        store
            .black_accounts(std::slice::from_ref(&acc))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!store.has_black_workspace(&acc).await.unwrap());

    let mut conn = t.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `billing` SET `subscription_id` = 'sub_fixture' WHERE `workspace_id` = ?",
        (&ws,),
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .black_accounts(std::slice::from_ref(&acc))
            .await
            .unwrap(),
        vec![acc.clone()]
    );
    assert!(store.has_black_workspace(&acc).await.unwrap());
    assert_eq!(
        store.last_seen_workspace(&acc).await.unwrap(),
        Some(ws.clone())
    );

    // Migrated workspace: admission sees `migrated`, last-seen excludes it.
    conn.exec_drop(
        "UPDATE `workspace` SET `migrated_at` = now() WHERE `id` = ?",
        (&ws,),
    )
    .await
    .unwrap();
    let member = store
        .admission_member(&ws, std::slice::from_ref(&acc))
        .await
        .unwrap()
        .unwrap();
    assert!(member.migrated);
    assert_eq!(store.last_seen_workspace(&acc).await.unwrap(), None);
    drop(conn);
    t.drop_database().await.unwrap();
}
