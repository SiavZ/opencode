//! Purpose: verify enterprise delivery against a loopback HTTP capture server.
//! Scope: fixture-only Salesforce OAuth, lead creation, EmailOctopus and SES wire acceptance.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/support/enterprise.rs
//! External:
//! |-- axum::{Router, extract::{Request, State}, response::IntoResponse}
//! |-- serde_json::{Value, json}
//! `-- std::sync::{Arc, Mutex}
//! Used by:
//! `-- none, entrypoint
//! Invariants: all endpoints use a loopback IP; no actual email, CRM or subscription side effects.
//! Contract tests: this file.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Request, State};
use axum::response::IntoResponse;
use opencode_hosted_adapters::console::support::enterprise::{
    EnterpriseClient, EnterpriseConfig, Inquiry,
};
use serde_json::{Value, json};

type Captures = Arc<Mutex<Vec<(String, String, axum::http::HeaderMap, Vec<u8>)>>>;

async fn capture(
    State((captures, fail)): State<(Captures, bool)>,
    request: Request,
) -> axum::response::Response {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    let headers = request.headers().clone();
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    captures
        .lock()
        .unwrap()
        .push((method, path.clone(), headers, body));
    if fail {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "fixture rejected",
        )
            .into_response();
    }
    axum::Json(if path.ends_with("/token") {
        json!({"access_token":"fixture-oauth-token"})
    } else {
        json!({"success":true})
    })
    .into_response()
}

async fn fixture(
    fail: bool,
) -> (
    EnterpriseConfig,
    Captures,
    tokio::task::JoinHandle<()>,
    tokio_util::sync::CancellationToken,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let captures: Captures = Default::default();
    let url = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let router = Router::new()
        .fallback(capture)
        .with_state((captures.clone(), fail));
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let mut config = EnterpriseConfig::default().with_local_base(url).unwrap();
    config.salesforce_client_id = Some("fixture-client".into());
    config.salesforce_client_secret = Some("fixture-secret".into());
    config.octopus_api_key = Some("fixture-octopus".into());
    config.sales_inbox = Some("sales@example.test".into());
    config.ses_access_key = Some("fixture-ses-key".into());
    config.ses_secret_key = Some("fixture-ses-secret".into());
    (config, captures, task, stop)
}

fn inquiry() -> Inquiry {
    Inquiry {
        name: "  Ada   Lovelace Byron  ".into(),
        role: "Engineer".into(),
        company: Some("  ".into()),
        email: "ada@example.test".into(),
        phone: None,
        message: "Private enterprise inquiry".into(),
    }
}

#[tokio::test]
async fn fixture_newsletter_preserves_non_success_and_transport_behavior() {
    for fail in [false, true] {
        let (config, captures, task, stop) = fixture(fail).await;
        assert!(
            EnterpriseClient::new(config)
                .unwrap()
                .email_signup(&json!("reader@example.test"))
                .await
                .unwrap()
        );
        let captured = captures.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "PUT");
        assert_eq!(
            captured[0].1,
            "/lists/8b9bb82c-9d5f-11f0-975f-0df6fd1e4945/contacts"
        );
        assert_eq!(captured[0].2["authorization"], "Bearer fixture-octopus");
        assert_eq!(
            serde_json::from_slice::<Value>(&captured[0].3).unwrap(),
            json!({"email_address":"reader@example.test"})
        );
        stop.cancel();
        task.await.unwrap();
    }
    let (config, _, task, stop) = fixture(false).await;
    stop.cancel();
    task.await.unwrap();
    assert_eq!(
        EnterpriseClient::new(config)
            .unwrap()
            .email_signup(&json!("reader@example.test"))
            .await
            .unwrap_err(),
        "Email signup request failed"
    );
}

#[tokio::test]
async fn fixture_enterprise_attempts_all_channels_with_baseline_payloads() {
    let (config, captures, task, stop) = fixture(false).await;
    let debug = format!("{config:?}");
    for secret in [
        "fixture-secret",
        "fixture-octopus",
        "fixture-ses-secret",
        "sales@example.test",
    ] {
        assert!(!debug.contains(secret));
    }
    let delivery = EnterpriseClient::new(config)
        .unwrap()
        .deliver(&inquiry())
        .await;
    assert!(delivery.lead && delivery.mail && delivery.octopus && delivery.accepted());
    let captured = captures.lock().unwrap().clone();
    assert_eq!(captured.len(), 4);
    let oauth = captured
        .iter()
        .find(|(_, path, _, _)| path.ends_with("/token"))
        .unwrap();
    assert_eq!(oauth.0, "POST");
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(&oauth.3).into_owned().collect();
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["client_secret"], "fixture-secret");
    let lead = captured
        .iter()
        .find(|(_, path, _, _)| path.ends_with("/Lead"))
        .unwrap();
    assert_eq!(lead.2["authorization"], "Bearer fixture-oauth-token");
    let lead: Value = serde_json::from_slice(&lead.3).unwrap();
    assert_eq!(lead["Company"], "Website");
    assert_eq!(lead["Phone"], Value::Null);
    assert_eq!(lead["LeadSource"], "Website");
    let octopus = captured
        .iter()
        .find(|(_, path, _, _)| path.ends_with("/contacts"))
        .unwrap();
    assert_eq!(octopus.0, "PUT");
    assert_eq!(octopus.2["authorization"], "Bearer fixture-octopus");
    assert_eq!(
        serde_json::from_slice::<Value>(&octopus.3).unwrap(),
        json!({"email_address":"ada@example.test","fields":{"FirstName":"Ada","LastName":"Lovelace Byron"}})
    );
    let mail = captured
        .iter()
        .find(|(_, path, _, _)| path.ends_with("/outbound-emails"))
        .unwrap();
    assert!(
        mail.2["authorization"]
            .to_str()
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256")
    );
    assert_eq!(mail.2["x-amz-target"], "SES.SendEmail");
    let mail: Value = serde_json::from_slice(&mail.3).unwrap();
    assert_eq!(mail["ReplyToAddresses"], json!(["ada@example.test"]));
    assert_eq!(
        mail["Destination"]["ToAddresses"],
        json!(["sales@example.test"])
    );
    assert_eq!(
        mail["Content"]["Simple"]["Subject"]["Data"],
        "Enterprise Inquiry from   Ada   Lovelace Byron  "
    );
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn fixture_enterprise_reports_all_channel_failures_and_rejects_remote_override() {
    assert!(
        EnterpriseConfig::default()
            .with_local_base(url::Url::parse("https://example.com").unwrap())
            .is_err()
    );
    let (config, captures, task, stop) = fixture(true).await;
    let delivery = EnterpriseClient::new(config)
        .unwrap()
        .deliver(&inquiry())
        .await;
    assert!(!delivery.accepted());
    assert_eq!(captures.lock().unwrap().len(), 3);
    stop.cancel();
    task.await.unwrap();
    assert!(
        !EnterpriseClient::new(EnterpriseConfig::default())
            .unwrap()
            .deliver(&inquiry())
            .await
            .accepted()
    );
}
