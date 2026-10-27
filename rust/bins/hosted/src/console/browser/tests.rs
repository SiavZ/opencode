//! Purpose: test browser transport route allowlists and origin rejection.
//! Scope: local unit HTTP transport checks without external services.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/browser.rs
//! External:
//! |-- axum::{body::Body, http::Request}
//! |-- serde_json::json
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/browser.rs
//! Invariants: identifiers cannot escape allowlisted tenant routes; browser origin is enforced.
//! Contract tests: this file.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local MySQL and installed Chromium; run explicitly for browser acceptance"]
async fn fixture_browser_ssr_and_action_round_trip() {
    use mysql_async::prelude::Queryable;
    use opencode_hosted_adapters::console::auth::SessionCodec;
    use opencode_hosted_adapters::console::db::TestDatabase;
    let fixture = TestDatabase::create("browser_transport").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account_id = store.create_account(None).await.unwrap();
    let actor = Actor::Account {
        account_id: account_id.clone(),
        email: "browser@example.test".into(),
    };
    let workspace_id = store
        .create_workspace(&actor, "Browser fixture workspace")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `billing` SET `subscription_id`='sub_browser_fixture' WHERE `workspace_id`=?",
        (&workspace_id,),
    )
    .await
    .unwrap();
    store
        .upsert_auth_links(
            &account_id,
            opencode_hosted_domain::console::identity::AuthProvider::Email,
            "browser@example.test",
            "browser@example.test",
        )
        .await
        .unwrap();
    drop(conn);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rust_url = format!("http://{}", listener.local_addr().unwrap());
    let browser_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let browser_port = browser_probe.local_addr().unwrap().port();
    drop(browser_probe);
    let origin = format!("http://127.0.0.1:{browser_port}");
    let ssr_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let ssr_port = ssr_probe.local_addr().unwrap().port();
    drop(ssr_probe);
    let secret = "browser-fixture-session-secret-at-least-32-bytes";
    let config = super::super::state::ConsoleConfig {
        auth: super::super::auth::Config {
            session_secret: Some(secret.into()),
            public_origin: Some(origin.clone()),
            console_url: Some(origin.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let router = super::super::router(ConsoleState {
        db: fixture.db.clone(),
        config: std::sync::Arc::new(config),
    });
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap()
    });
    let codec = SessionCodec::new(secret);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut session = codec.load(None, now);
    session.data.login(&account_id, "browser@example.test");
    let cookie = codec.set_cookie(&session, now).unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/console-browser/acceptance.ts");
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bun")
            .arg(script)
            .env("OPENCODE_BROWSER_RUST_URL", rust_url)
            .env("OPENCODE_CONSOLE_PUBLIC_ORIGIN", origin)
            .env("OPENCODE_BROWSER_PORT", browser_port.to_string())
            .env("OPENCODE_SSR_PORT", ssr_port.to_string())
            .env("OPENCODE_BROWSER_TEST_WORKSPACE", workspace_id)
            .env("OPENCODE_BROWSER_TEST_COOKIE", cookie)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    shutdown.cancel();
    server.await.unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    let workspaces = store.account_workspaces(&account_id).await.unwrap();
    let persisted = workspaces
        .iter()
        .any(|row| row.name == "Browser action persisted");
    fixture.drop_database().await.unwrap();
    assert!(output.status.success(), "Browser acceptance process failed");
    assert!(
        persisted,
        "Actual browser mutation did not persist in MySQL"
    );
}

#[test]
fn all_browser_operations_have_concrete_allowlisted_targets() {
    let operations = [
        "workspaces",
        "workspace.create",
        "workspace.get",
        "workspace.update",
        "key.list",
        "key.create",
        "key.remove",
        "member.list",
        "member.create",
        "member.update",
        "member.remove",
        "billing.get",
        "payment.list",
        "usage.list",
        "checkoutUrl",
        "sessionUrl",
        "liteSessionUrl",
        "receipt.download",
        "billing.reload",
        "billing.setReload",
        "billing.setMonthlyLimit",
        "billing.redeemCoupon",
        "subscription.get",
        "cancelWaitlist",
        "enroll",
        "setUseBalance",
        "lite.subscription.get",
        "lite.subscription.usage",
        "liteCheckoutUrl",
        "setLiteUseBalance",
        "go.providerRouting.set",
        "go.allowTraining.set",
        "provider.list",
        "provider.save",
        "provider.remove",
        "model.info",
        "model.toggle",
        "getCosts",
        "go.referral.get",
        "go.referral.usagePreview",
        "go.referral.reward.apply",
        "go.referral.capture",
        "getBenchmarks",
        "getTaskDetail",
        "github",
        "changelog",
        "emailSignup",
        "auth.logout",
    ];
    for operation in operations {
        let call: Call = serde_json::from_value(json!({"operation":operation,"workspace":"wrk_fixture","resource":"item_123","input":{"window":"rolling","year":2026,"month":9,"tzOffset":"+02:00"}})).unwrap();
        let (_, target) =
            target(&call).unwrap_or_else(|response| panic!("{operation}: {}", response.status()));
        assert!(target.starts_with('/'), "{operation}");
        assert!(
            !target.contains("/_console/browser"),
            "recursive browser dispatch"
        );
    }
}

#[tokio::test]
async fn allowlisted_actions_reach_actual_handlers_not_missing_routes() {
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/browser_unused",
    )
    .unwrap();
    let state = ConsoleState {
        db,
        config: std::sync::Arc::new(super::super::state::ConsoleConfig {
            auth: super::super::auth::Config {
                session_secret: Some("browser-route-probe-secret-at-least-32-bytes".into()),
                public_origin: Some("http://127.0.0.1:3001".into()),
                console_url: Some("http://127.0.0.1:3001".into()),
                ..Default::default()
            },
            ..Default::default()
        }),
    };
    let router = routes().with_state(state);
    for operation in [
        "workspaces",
        "workspace.create",
        "workspace.get",
        "workspace.update",
        "key.list",
        "key.create",
        "key.remove",
        "member.list",
        "member.create",
        "member.update",
        "member.remove",
        "billing.get",
        "payment.list",
        "usage.list",
        "checkoutUrl",
        "sessionUrl",
        "liteSessionUrl",
        "receipt.download",
        "billing.reload",
        "billing.setReload",
        "billing.setMonthlyLimit",
        "billing.redeemCoupon",
        "subscription.get",
        "cancelWaitlist",
        "enroll",
        "setUseBalance",
        "lite.subscription.get",
        "lite.subscription.usage",
        "liteCheckoutUrl",
        "setLiteUseBalance",
        "go.providerRouting.set",
        "go.allowTraining.set",
        "provider.list",
        "provider.save",
        "provider.remove",
        "model.info",
        "model.toggle",
        "getCosts",
        "go.referral.get",
        "go.referral.usagePreview",
        "go.referral.reward.apply",
        "go.referral.capture",
    ] {
        let response = router.clone().oneshot(Request::post("/_console/browser").header("content-type", "application/json").header("origin", "http://127.0.0.1:3001").body(Body::from(json!({"operation":operation,"workspace":"wrk_fixture","resource":"item_123","input":{"window":"rolling","year":2026,"month":9,"tzOffset":"+02:00"}}).to_string())).unwrap()).await.unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "missing endpoint for {operation}"
        );
        assert_ne!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "wrong method for {operation}"
        );
        assert_ne!(
            response.status(),
            StatusCode::NOT_IMPLEMENTED,
            "unimplemented {operation}"
        );
    }
}

#[test]
fn browser_dispatch_rejects_paths_and_unknown_operations() {
    for workspace in ["../auth/logout", "wrk/id", "wrk?x=1", "", "wrk%2fother"] {
        let call: Call =
            serde_json::from_value(json!({"operation":"key.list","workspace":workspace})).unwrap();
        assert_eq!(target(&call).unwrap_err().status(), StatusCode::BAD_REQUEST);
    }
    let call: Call =
        serde_json::from_value(json!({"operation":"arbitrary","workspace":"wrk_123"})).unwrap();
    assert_eq!(
        target(&call).unwrap_err().status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let call: Call = serde_json::from_value(
        json!({"operation":"key.remove","workspace":"wrk_123","resource":"key_456"}),
    )
    .unwrap();
    assert_eq!(
        target(&call).unwrap(),
        (Method::DELETE, "/workspaces/wrk_123/keys/key_456".into())
    );
}

#[tokio::test]
async fn browser_dispatch_rejects_cross_origin_before_database() {
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/browser_unused",
    )
    .unwrap();
    let state = ConsoleState {
        db,
        config: std::sync::Arc::new(super::super::state::ConsoleConfig::default()),
    };
    let response = routes()
        .with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/_console/browser")
                .header("content-type", "application/json")
                .header("sec-fetch-site", "cross-site")
                .body(Body::from(r#"{"operation":"workspaces"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
