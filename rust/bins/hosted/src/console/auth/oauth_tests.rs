//! Purpose: exercise hosted OAuth authorize/callback with signed JWTs and real Redis over TCP.
//! Scope: isolated loopback issuer and console routers; Redis from OPENCODE_P12_REDIS_URL.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- axum::{Json, Router}
//! |-- jsonwebtoken::{Algorithm, EncodingKey, Header}
//! |-- reqwest::Client
//! `-- tokio::net::TcpListener
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/auth.rs
//! Invariants: no live issuer or database calls; Redis is an explicitly configured local fixture.
//! Contract tests: fixture_oauth_authorize_callback_signed_jwt_redis_binding_and_replay.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::{Form, Query};
use axum::http::{StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use opencode_hosted_adapters::console::db::ConsoleDb;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::Config;
use crate::console::state::{ConsoleConfig, ConsoleState};

const SECRET: &str = "hosted-oauth-fixture-session-secret-at-least-32-bytes";
const SIGNING_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgw7tDBuHtqIOB6FRR\nzJbG5AHmjP2dmnYddCN/3nE8RduhRANCAARPuKdPr3mreSJ9kiLkcz+02BWgFnA0\nsa2VMbNAn76sI5Wg4dQ2tB5AE9jAUwqZ1OnKSSstI9BcQODDGOngmrWI\n-----END PRIVATE KEY-----\n";

type Exchanges = Arc<tokio::sync::Mutex<Vec<HashMap<String, String>>>>;

struct Fixture {
    console: String,
    issuer: String,
    http: reqwest::Client,
    exchanges: Exchanges,
    discovery: Arc<AtomicUsize>,
    jwks: Arc<AtomicUsize>,
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

impl Fixture {
    async fn start() -> Self {
        let redis = std::env::var("OPENCODE_P12_REDIS_URL")
            .expect("set OPENCODE_P12_REDIS_URL to an isolated local real Redis fixture");
        let redis_url = url::Url::parse(&redis).unwrap();
        assert_eq!(redis_url.scheme(), "redis");
        assert!(redis_url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }));
        let issuer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", issuer_listener.local_addr().unwrap());
        let console_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let console = format!("http://{}", console_listener.local_addr().unwrap());
        let key = EncodingKey::from_ec_pem(SIGNING_KEY).unwrap();
        let mut jwk = jsonwebtoken::jwk::Jwk::from_encoding_key(&key, Algorithm::ES256).unwrap();
        jwk.common.key_id = Some("hosted-oauth-fixture".into());
        jwk.common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::ES256);
        jwk.common.public_key_use = Some(jsonwebtoken::jwk::PublicKeyUse::Signature);
        let document = json!({"keys": [jwk]});
        let discovery = Arc::new(AtomicUsize::new(0));
        let jwks = Arc::new(AtomicUsize::new(0));
        let exchanges: Exchanges = Arc::default();
        let app = Router::new()
            .route("/authorize", get({
                let console = console.clone();
                move |Query(query): Query<HashMap<String, String>>| {
                    let console = console.clone();
                    async move {
                        assert_eq!(query["client_id"], "app");
                        assert_eq!(query["response_type"], "code");
                        let mut callback = url::Url::parse(&query["redirect_uri"]).unwrap();
                        assert_eq!(callback.origin().ascii_serialization(), console);
                        callback.query_pairs_mut()
                            .append_pair("code", "fixture-code")
                            .append_pair("state", &query["state"]);
                        (StatusCode::FOUND, [(header::LOCATION, callback.to_string())])
                    }
                }
            }))
            .route("/.well-known/oauth-authorization-server", get({
                let issuer = issuer.clone();
                let discovery = discovery.clone();
                move || {
                    let issuer = issuer.clone();
                    let discovery = discovery.clone();
                    async move {
                        discovery.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"issuer": issuer, "jwks_uri": format!("{issuer}/jwks")}))
                    }
                }
            }))
            .route("/jwks", get({
                let jwks = jwks.clone();
                move || {
                    let document = document.clone();
                    let jwks = jwks.clone();
                    async move {
                        jwks.fetch_add(1, Ordering::SeqCst);
                        Json(document)
                    }
                }
            }))
            .route("/token", post({
                let issuer = issuer.clone();
                let exchanges = exchanges.clone();
                move |Form(form): Form<HashMap<String, String>>| {
                    let issuer = issuer.clone();
                    let exchanges = exchanges.clone();
                    async move {
                        let code = form["code"].clone();
                        exchanges.lock().await.push(form);
                        if code == "exchange-failure" {
                            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"})));
                        }
                        let now = time::OffsetDateTime::now_utc().unix_timestamp();
                        let mut header = Header::new(Algorithm::ES256);
                        header.kid = Some("hosted-oauth-fixture".into());
                        let mut token = jsonwebtoken::encode(&header, &json!({
                            "iss": if code == "wrong-issuer" { "http://wrong.example" } else { &issuer },
                            "aud": if code == "wrong-audience" { "other-app" } else { "app" },
                            "exp": if code == "expired" { now - 60 } else { now + 300 },
                            "sub": "account:hosted-fixture",
                            "mode": "access", "type": "account",
                            "properties": {"accountID": "acc_oauth_fixture", "email": "oauth@example.test"}
                        }), &EncodingKey::from_ec_pem(SIGNING_KEY).unwrap()).unwrap();
                        if code == "tampered-jwt" {
                            let index = token.rfind('.').unwrap() + 1;
                            let replacement = if &token[index..index + 1] == "A" { "B" } else { "A" };
                            token.replace_range(index..index + 1, replacement);
                        }
                        (StatusCode::OK, Json(json!({"access_token": token, "token_type": "Bearer"})))
                    }
                }
            }));
        let state = ConsoleState {
            config: Arc::new(ConsoleConfig {
                auth: Config {
                    issuer_url: Some(issuer.clone()),
                    session_secret: Some(SECRET.into()),
                    state_redis_url: Some(redis),
                    public_origin: Some(console.clone()),
                    console_url: None,
                },
                ..Default::default()
            }),
            // These routes must not query MySQL. No database service is needed by this fixture.
            db: ConsoleDb::connect("mysql://unused:unused@127.0.0.1:1/unused").unwrap(),
        };
        let cancel = CancellationToken::new();
        let tasks = [
            (issuer_listener, app),
            (console_listener, crate::console::router(state)),
        ]
        .into_iter()
        .map(|(listener, app)| {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(cancel.cancelled_owned())
                    .await
                    .unwrap();
            })
        })
        .collect();
        Self {
            console,
            issuer,
            exchanges,
            discovery,
            jwks,
            cancel,
            tasks,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    async fn authorize(&self, continuation: &str) -> (url::Url, String) {
        let mut authorize = url::Url::parse(&format!("{}/auth/authorize", self.console)).unwrap();
        authorize
            .query_pairs_mut()
            .append_pair("continue", continuation);
        let response = self
            .http
            .get(authorize)
            .header("x-forwarded-host", "attacker.example")
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let cookie = cookie(&response);
        let location =
            url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(location.origin().ascii_serialization(), self.issuer);
        assert_eq!(location.path(), "/authorize");
        let response = self.http.get(location).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let callback =
            url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(callback.origin().ascii_serialization(), self.console);
        (callback, cookie)
    }

    async fn reject(&self, callback: &url::Url, cookie: Option<&str>, expected: &str) {
        let before = self.exchanges.lock().await.len();
        let request = self.http.get(callback.clone());
        let request = match cookie {
            Some(cookie) => request.header(header::COOKIE, cookie),
            None => request,
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(!response.headers().contains_key(header::LOCATION));
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], expected);
        assert_eq!(body["cause"], json!({}));
        assert!(!body.to_string().contains("fixture-code"));
        if expected == "Invalid authorization state" {
            assert_eq!(self.exchanges.lock().await.len(), before);
        }
    }

    async fn stop(mut self) {
        self.cancel.cancel();
        for task in self.tasks.drain(..) {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancel.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn cookie(response: &reqwest::Response) -> String {
    let value = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(value.starts_with("auth=Fe26.2*"));
    assert!(value.contains("; Path=/;"));
    assert!(value.contains("; Expires="));
    assert!(value.ends_with("; HttpOnly"));
    value.split(';').next().unwrap().to_string()
}

fn with_code(callback: &url::Url, code: &str) -> url::Url {
    let mut next = callback.clone();
    next.set_query(None);
    next.query_pairs_mut()
        .extend_pairs(callback.query_pairs().filter(|(key, _)| key != "code"))
        .append_pair("code", code);
    next
}

#[tokio::test]
async fn fixture_oauth_authorize_callback_signed_jwt_redis_binding_and_replay() {
    let fixture = Fixture::start().await;
    let (callback, pending_cookie) = fixture.authorize("/workspace/wrk_oauth_fixture").await;
    assert_eq!(
        callback.path(),
        "/auth/callback/workspace/wrk_oauth_fixture"
    );
    let (default_callback, other_cookie) = fixture.authorize("").await;
    assert_eq!(default_callback.path(), "/auth/callback");
    assert_ne!(callback.query(), default_callback.query());
    fixture
        .reject(&callback, None, "Invalid authorization state")
        .await;
    fixture
        .reject(
            &callback,
            Some(&other_cookie),
            "Invalid authorization state",
        )
        .await;
    let mut tampered_cookie = pending_cookie.clone();
    let last = tampered_cookie.pop().unwrap();
    tampered_cookie.push(if last == 'A' { 'B' } else { 'A' });
    fixture
        .reject(
            &callback,
            Some(&tampered_cookie),
            "Invalid authorization state",
        )
        .await;
    let mut tampered_state = callback.clone();
    tampered_state.set_query(None);
    tampered_state
        .query_pairs_mut()
        .append_pair("code", "fixture-code")
        .append_pair("state", "00000000-0000-4000-8000-000000000000");
    fixture
        .reject(
            &tampered_state,
            Some(&pending_cookie),
            "Invalid authorization state",
        )
        .await;
    assert_eq!(fixture.discovery.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.jwks.load(Ordering::SeqCst), 0);

    let response = fixture
        .http
        .get(callback.clone())
        .header(header::COOKIE, &pending_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/workspace/wrk_oauth_fixture"
    );
    let authenticated = cookie(&response);
    assert_ne!(authenticated, pending_cookie);
    let response = fixture
        .http
        .get(format!("{}/auth/status", fixture.console))
        .header(header::COOKIE, &authenticated)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({
            "current": "acc_oauth_fixture", "account": {
                "acc_oauth_fixture": {"id": "acc_oauth_fixture", "email": "oauth@example.test"}
            }
        })
    );
    assert_eq!(fixture.discovery.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.jwks.load(Ordering::SeqCst), 1);
    let exchanges = fixture.exchanges.lock().await;
    assert_eq!(exchanges.len(), 1);
    assert_eq!(
        exchanges[0],
        HashMap::from([
            ("code".into(), "fixture-code".into()),
            (
                "redirect_uri".into(),
                format!(
                    "{}/auth/callback/workspace/wrk_oauth_fixture",
                    fixture.console
                )
            ),
            ("grant_type".into(), "authorization_code".into()),
            ("client_id".into(), "app".into()),
            ("code_verifier".into(), "".into()),
        ])
    );
    drop(exchanges);
    fixture
        .reject(
            &callback,
            Some(&pending_cookie),
            "Invalid authorization state",
        )
        .await;
    fixture
        .reject(
            &callback,
            Some(&authenticated),
            "Invalid authorization state",
        )
        .await;

    let response = fixture
        .http
        .get(default_callback.clone())
        .header(header::COOKIE, &other_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/auth");
    cookie(&response);
    fixture
        .reject(
            &default_callback,
            Some(&other_cookie),
            "Invalid authorization state",
        )
        .await;

    for code in [
        "tampered-jwt",
        "wrong-issuer",
        "wrong-audience",
        "expired",
        "exchange-failure",
    ] {
        let (callback, pending) = fixture.authorize("").await;
        let callback = with_code(&callback, code);
        let expected = if code == "exchange-failure" {
            "Authorization code exchange failed"
        } else {
            "Invalid access token"
        };
        fixture.reject(&callback, Some(&pending), expected).await;
        fixture
            .reject(&callback, Some(&pending), "Invalid authorization state")
            .await;
        let response = fixture
            .http
            .get(format!("{}/auth/status", fixture.console))
            .header(header::COOKIE, pending)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap(), json!({}));
    }
    assert_eq!(fixture.exchanges.lock().await.len(), 7);
    let (callback, pending) = fixture.authorize("").await;
    let requests = [0, 1].map(|_| {
        fixture
            .http
            .get(callback.clone())
            .header(header::COOKIE, &pending)
            .send()
    });
    let [first, second] = requests;
    let (first, second) = tokio::join!(first, second);
    let responses = [first.unwrap(), second.unwrap()];
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.status() == StatusCode::FOUND)
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.status() == StatusCode::INTERNAL_SERVER_ERROR)
            .count(),
        1
    );
    for response in responses {
        if response.status() == StatusCode::FOUND {
            cookie(&response);
            continue;
        }
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "Invalid authorization state"
        );
    }
    assert_eq!(fixture.exchanges.lock().await.len(), 8);
    fixture.stop().await;
}
