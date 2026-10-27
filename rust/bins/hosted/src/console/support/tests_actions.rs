//! Purpose: prove quota, referral and operator lookup routes persist baseline behavior end to end.
//! Scope: Axum acceptance with baseline-migrated isolated MySQL.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/support/tests.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- axum::http::StatusCode
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: only local fixture rows are changed; unauthorized lookup never opens SQL.
//! Contract tests: this file.

use axum::http::StatusCode;
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use serde_json::json;

use super::tests::{AUTH, app, config, request};

#[tokio::test]
async fn support_enterprise_accepts_one_channel_and_honeypot_never_delivers() {
    use opencode_hosted_adapters::console::support::enterprise::EnterpriseConfig;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let sink = count.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(move || {
                let sink = sink.clone();
                async move {
                    sink.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .unwrap();
    });
    let mut enterprise = EnterpriseConfig::default().with_local_base(url).unwrap();
    enterprise.octopus_api_key = Some("fixture-octopus".into());
    let support = super::Config {
        enterprise,
        ..config()
    };
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/offline",
    )
    .unwrap();
    let payload =
        r#"{"name":"Ada","role":"Engineer","email":"ada@example.test","message":"Interested"}"#;
    assert_eq!(
        request(
            app(db.clone(), support.clone(), "production"),
            "POST",
            "/api/enterprise",
            &[],
            payload
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        request(
            app(db.clone(), support.clone(), "production"),
            "POST",
            "/api/email-signup",
            &[],
            r#"{"email":"reader@example.test"}"#
        )
        .await,
        (StatusCode::OK, json!(true))
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        request(
            app(db, support, "production"),
            "POST",
            "/api/enterprise",
            &[],
            r#"{"alias":"bot"}"#
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    stop.cancel();
    server.await.unwrap();
}

#[test]
fn support_honeycomb_variants_and_locale_precedence() {
    for kind in ["model_http_errors", "provider_http_errors"] {
        assert_eq!(super::honeycomb::discord_content(&json!({"type":kind,"url":"url","groups":[{"result":0.125,"group":[{"key":"model","value":"a"},{"key":"provider","value":"b"}]}]})).unwrap(),"[**Honeycomb alert**](url)\n- a (13% errors)\n- b (13% errors)\n\n<@&1520924666359713863>");
    }
    assert_eq!(
        super::honeycomb::discord_content(&json!({"type":"custom","url":"url"})).unwrap(),
        "[**Honeycomb alert**](url)\n\n<@&1520924666359713863>"
    );
    assert!(
        super::honeycomb::discord_content(
            &json!({"type":"model_low_tps","url":"url","groups":[{"result":true,"group":[]}]})
        )
        .is_none()
    );
    assert!(
        super::honeycomb::discord_content(&json!({"type":"custom","url":"url","isTest":"yes"}))
            .is_none()
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("accept-language", "en;q=0.1,fr;q=0.9".parse().unwrap());
    assert_eq!(
        super::locale::messages(&headers)[0],
        "Formulaire soumis avec succès."
    );
    headers.insert("cookie", "oc_locale=ja".parse().unwrap());
    assert_eq!(
        super::locale::messages(&headers)[0],
        "フォームが正常に送信されました。"
    );
    headers.insert("x-opencode-locale", "de".parse().unwrap());
    assert_eq!(
        super::locale::messages(&headers)[0],
        "Formular erfolgreich gesendet."
    );
}

#[tokio::test]
async fn support_validation_reports_root_and_indexed_schema_errors() {
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/offline",
    )
    .unwrap();
    let (status, body) = request(
        app(db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/block-workspaces",
        AUTH,
        r#"{"workspaceIDs":[42,"bad"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["issues"][0],
        json!({"expected":"string","code":"invalid_type","path":["workspaceIDs",0],"message":"Invalid input: expected string, received number"})
    );
    assert_eq!(body["issues"][1]["path"], json!(["workspaceIDs", 1]));
    let (status, body) = request(
        app(db, config(), "production"),
        "DELETE",
        "/api/support/actions/delete-account",
        AUTH,
        "null",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["issues"],
        json!([{"expected":"object","code":"invalid_type","path":[],"message":"Invalid input: expected object, received null"}])
    );
}

#[tokio::test]
async fn support_honeycomb_reports_discord_rejection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/discord", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .unwrap();
    });
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/offline",
    )
    .unwrap();
    let support = super::Config {
        discord_webhook_url: Some(url),
        ..config()
    };
    assert_eq!(
        request(
            app(db, support, "production"),
            "POST",
            "/honeycomb/webhook",
            &[("x-honeycomb-webhook-token", "fixture-honey-secret")],
            r#"{"type":"custom","status":"TRIGGERED","url":"url"}"#
        )
        .await,
        (
            StatusCode::BAD_GATEWAY,
            json!({"message":"discord webhook failed"})
        )
    );
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
async fn fixture_support_reset_referral_and_lookup_routes() {
    let fixture = TestDatabase::create("support_remaining_http")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name) VALUES ('wrk_inviter','Inviter'),('wrk_invitee','Invitee')").await.unwrap();
    conn.query_drop("INSERT INTO account (id) VALUES ('acc_invitee')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO auth (id,provider,subject,account_id) VALUES ('aut_invitee','email','invitee@example.test','acc_invitee')").await.unwrap();
    conn.query_drop("INSERT INTO `user` (id,workspace_id,account_id,name,role) VALUES ('usr_invitee','wrk_invitee','acc_invitee','Invitee','admin')").await.unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance) VALUES ('bil_invitee','wrk_invitee',123000000)").await.unwrap();
    conn.query_drop("INSERT INTO lite (id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage) VALUES ('lit_invitee','wrk_invitee','usr_invitee',1,2,3)").await.unwrap();
    let (status, result) = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/reset-quota",
        AUTH,
        r#"{"workspaceID":"wrk_invitee","plan":"lite"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result,
        json!({"success":true,"message":"Usage counters reset to zero","result":{"plan":"lite","before":{"rollingUsage":1,"weeklyUsage":2,"monthlyUsage":3}}})
    );
    let payload = r#"{"inviterWorkspaceID":"wrk_inviter","inviteeWorkspaceID":"wrk_invitee"}"#;
    let (status, result) = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/create-referral",
        AUTH,
        payload,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["result"]["createdRewards"], 2);
    let repeated = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/create-referral",
        AUTH,
        payload,
    )
    .await;
    assert_eq!(repeated.1["result"]["createdRewards"], 0);
    assert_eq!(
        repeated.1["result"]["referralID"],
        result["result"]["referralID"]
    );
    let mut configured = config();
    configured.limits = Some(
        json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":50,"monthlyLimit":100},"black":{"20":{"fixedLimit":20,"rollingLimit":5,"rollingWindow":5},"100":{"fixedLimit":100,"rollingLimit":25,"rollingWindow":5},"200":{"fixedLimit":200,"rollingLimit":50,"rollingWindow":5}}}),
    );
    let (status, result) = request(
        app(fixture.db.clone(), configured.clone(), "production"),
        "GET",
        "/api/support/lookup?identifier=wrk_invitee",
        AUTH,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["workspaces"][0]["billing"]["balance"], "$1.23");
    assert_eq!(
        result["workspaces"][0]["go"][0]["email"],
        "invitee@example.test"
    );
    assert_eq!(
        request(
            app(fixture.db.clone(), configured, "production"),
            "GET",
            "/api/support/lookup?identifier=wrk_invitee",
            &[],
            ""
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app(fixture.db.clone(), config(), "production"),
            "GET",
            "/api/support/lookup?identifier=wrk_invitee",
            AUTH,
            ""
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let ids = (0..501)
        .map(|index| format!("wrk_chunk_{index:04}"))
        .collect::<Vec<_>>();
    conn.exec_batch(
        "INSERT INTO workspace (id,name) VALUES (?,'Chunk fixture')",
        ids.iter().map(|id| (id,)),
    )
    .await
    .unwrap();
    let mut requested = ids.clone();
    requested.push(ids[0].clone());
    requested.push("wrk_chunk_missing".into());
    let (status, result) = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/block-workspaces",
        AUTH,
        &json!({"workspaceIDs":requested}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["result"]["blocked"], json!(ids));
    assert_eq!(result["result"]["notFound"], json!(["wrk_chunk_missing"]));
    drop(conn);
    fixture.drop_database().await.unwrap();
}
