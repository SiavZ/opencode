//! Purpose: build disposable MySQL and signed-session workspace settings fixtures.
//! Scope: acceptance setup and HTTP request helper.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! Invariants: tests never contact real external providers or production data.
//! Contract tests: workspace/settings/tests.rs

use super::*;
use crate::console::state::ConsoleConfig;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use opencode_hosted_adapters::console::auth::SessionCodec;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_domain::console::identity::AuthProvider;
use tower::ServiceExt;

const SECRET: &str = "workspace-settings-test-session-secret-at-least-32-bytes";

pub(super) struct Fixture {
    pub(super) database: TestDatabase,
    pub(super) router: Router,
    pub(super) workspace: String,
    pub(super) other: String,
    pub(super) owner: String,
    pub(super) member: String,
    pub(super) outsider: String,
}

fn cookie(account: &str) -> String {
    let codec = SessionCodec::new(SECRET);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut session = codec.load(None, now);
    session.data.login(account, "owner@example.test");
    codec
        .set_cookie(&session, now)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}

impl Fixture {
    pub(super) async fn new(label: &str) -> Self {
        let database = TestDatabase::create(label).await.unwrap();
        let store = TenantStore::new(database.db.clone());
        let owner = store.create_account(None).await.unwrap();
        store
            .upsert_auth_links(
                &owner,
                AuthProvider::Github,
                "settings-owner",
                "owner@example.test",
            )
            .await
            .unwrap();
        let actor = Actor::Account {
            account_id: owner.clone(),
            email: "owner@example.test".into(),
        };
        let workspace = store.create_workspace(&actor, "Settings").await.unwrap();
        let other = store
            .create_workspace(&actor, "Other settings")
            .await
            .unwrap();
        let member = store.create_account(None).await.unwrap();
        let outsider = store.create_account(None).await.unwrap();
        let mut conn = database.db.conn().await.unwrap();
        conn.exec_drop(
            "UPDATE `billing` SET `subscription_id` = CASE WHEN `workspace_id` = ? THEN 'sub_fixture_a' ELSE 'sub_fixture_b' END WHERE `workspace_id` IN (?, ?)",
            (&workspace, &workspace, &other),
        )
        .await
        .unwrap();
        conn.exec_drop("INSERT INTO `user` (`id`,`workspace_id`,`account_id`,`name`,`role`) VALUES ('usr_settings_member', ?, ?, '', 'member')", (&workspace, &member)).await.unwrap();
        drop(conn);
        let zen = crate::console::zen::Config::from_catalog(json!({
            "liteModels": {}, "zenModels": {
                "gpt-z": {"name":"Zebra"}, "claude-b": [{"name":"Beta"}],
                "claude-a": {"name":"Alpha"}, "big-pickle": {"name":"Pickle"},
                "claude-3-5-haiku": {"name":"Excluded"}, "alpha-hidden": {"name":"Hidden"},
                "gpt-z:global": {"name":"Global"}, "unknown": {"name":"Unknown"}
            }
        }))
        .unwrap();
        let config = ConsoleConfig {
            auth: auth::Config {
                session_secret: Some(SECRET.into()),
                public_origin: Some(ORIGIN.into()),
                console_url: Some(ORIGIN.into()),
                ..Default::default()
            },
            zen,
            ..Default::default()
        };
        let router = crate::console::workspace::routes().with_state(ConsoleState {
            config: Arc::new(config),
            db: database.db.clone(),
        });
        Self {
            database,
            router,
            workspace,
            other,
            owner: cookie(&owner),
            member: cookie(&member),
            outsider: cookie(&outsider),
        }
    }

    pub(super) async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "console.opencode.ai")
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        let response = self
            .router
            .clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}
