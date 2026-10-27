//! Purpose: local issuer acceptance against actual HTTP fake providers and durable SQLite.
//! Scope: fixture-only; no live GitHub, Google or deployment calls.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- axum::Router
//! |-- jsonwebtoken::encode
//! `-- tempfile::TempDir
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! Invariants: test admission is invoked only after real provider verification succeeds.
//! Contract tests: this file

use super::*;

mod concurrency;
use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgw7tDBuHtqIOB6FRR\nzJbG5AHmjP2dmnYddCN/3nE8RduhRANCAARPuKdPr3mreSJ9kiLkcz+02BWgFnA0\nsa2VMbNAn76sI5Wg4dQ2tB5AE9jAUwqZ1OnKSSstI9BcQODDGOngmrWI\n-----END PRIVATE KEY-----\n";

struct Admission(AtomicUsize);

#[async_trait::async_trait]
impl AccountAdmission for Admission {
    async fn admit(&self, identity: &ProviderIdentity) -> Result<IssuerOutcome, Error> {
        let verified =
            opencode_hosted_domain::console::identity::resolve_identity(identity, "production")
                .map_err(|_| Error::Provider)?;
        assert_eq!(verified.email, "fixture@anoma.ly");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(IssuerOutcome::IssueAccount {
            account_id: "acc_fixture".into(),
            email: verified.email,
        })
    }
}

struct ProviderServer {
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl ProviderServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(|State(base): State<String>| async move { Json(json!({"issuer":base,"authorization_endpoint":format!("{base}/google-authorize"),"jwks_uri":format!("{base}/jwks")})) }))
            .route("/jwks", get(|| async {
                let key = jsonwebtoken::EncodingKey::from_ec_pem(KEY).unwrap();
                let mut jwk = serde_json::to_value(jsonwebtoken::jwk::Jwk::from_encoding_key(&key, jsonwebtoken::Algorithm::ES256).unwrap()).unwrap();
                jwk["kid"] = json!("provider-key"); jwk["alg"]=json!("ES256"); jwk["use"]=json!("sig");
                Json(json!({"keys":[jwk]}))
            }))
            .route("/token", post(|Form(form): Form<HashMap<String,String>>| async move {
                if form.get("code").map(String::as_str)!=Some("github-code") || form.get("client_secret").map(String::as_str)!=Some("secret") || form.get("redirect_uri").map(String::as_str)!=Some("https://auth.opencode.ai/github/callback") {
                    return (StatusCode::BAD_REQUEST,Json(json!({"error":"invalid_grant"})));
                }
                (StatusCode::OK,Json(json!({"access_token":"provider-only-token"})))
            }))
            .route("/user/emails", get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer provider-only-token");
                Json(json!([{"email":"fixture@anoma.ly","primary":true,"verified":true}]))
            }))
            .route("/user", get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer provider-only-token");
                Json(json!({"id":12345}))
            }))
            .with_state(base.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base, task }
    }

    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn runtime(path: &std::path::Path, base: &str) -> IssuerRuntime {
    IssuerRuntime::new(
        IssuerConfig {
            issuer: "https://auth.opencode.ai".into(),
            signing_key_pem: KEY.to_vec(),
            key_id: "fixture-signing-key".into(),
            retired_jwks: vec![],
            github: GithubConfig {
                client_id: "github-client".into(),
                client_secret: "secret".into(),
                authorization_endpoint: format!("{base}/github-authorize"),
                token_endpoint: format!("{base}/token"),
                api_base: base.into(),
            },
            google: GoogleConfig {
                client_id: "google-client".into(),
                issuer: base.into(),
            },
        },
        Arc::new(SqliteStore::open(path).unwrap()),
    )
    .unwrap()
}

fn authorize(provider: &str, pkce: bool) -> HashMap<String, String> {
    let mut query = HashMap::from([
        ("client_id".into(), "app".into()),
        (
            "redirect_uri".into(),
            "https://opencode.ai/auth/callback".into(),
        ),
        ("response_type".into(), "code".into()),
        ("state".into(), "client-state".into()),
        ("provider".into(), provider.into()),
    ]);
    if pkce {
        query.insert("code_challenge".into(), digest(&"v".repeat(43)));
        query.insert("code_challenge_method".into(), "S256".into());
    }
    query
}

fn flow(response: &BrowserResponse) -> (String, HashMap<String, String>) {
    let browser = response
        .cookie
        .as_ref()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("__Host-issuer_browser=")
        .unwrap()
        .to_string();
    let url = url::Url::parse(response.location.as_ref().unwrap()).unwrap();
    (browser, url.query_pairs().into_owned().collect())
}

fn exchange(response: &BrowserResponse) -> HashMap<String, String> {
    let destination = url::Url::parse(response.location.as_ref().unwrap()).unwrap();
    let query: HashMap<String, String> = destination.query_pairs().into_owned().collect();
    assert_eq!(query["state"], "client-state");
    HashMap::from([
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), query["code"].clone()),
        ("client_id".into(), "app".into()),
        (
            "redirect_uri".into(),
            "https://opencode.ai/auth/callback".into(),
        ),
        ("code_verifier".into(), "v".repeat(43)),
    ])
}

fn google_token(base: &str, nonce: &str, field: Option<(&str, Value)>) -> String {
    let mut claims = json!({"iss":base,"aud":"google-client","sub":"google-subject","email":"fixture@anoma.ly","email_verified":true,"iat":now(),"exp":now()+300,"nonce":nonce});
    if let Some((name, value)) = field {
        claims[name] = value;
    }
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some("provider-key".into());
    jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(KEY).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn github_code_pkce_refresh_and_restart_acceptance() {
    let server = ProviderServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("issuer.sqlite");
    let issuer = runtime(&path, &server.base);
    let admission = Admission(AtomicUsize::new(0));
    let start = issuer.authorize(&authorize("github", true)).await.unwrap();
    let (browser, provider) = flow(&start);
    assert_eq!(provider["scope"], "read:user user:email");
    assert!(
        start
            .cookie
            .as_ref()
            .unwrap()
            .contains("Secure; SameSite=None")
    );
    let callback = HashMap::from([
        ("state".into(), provider["state"].clone()),
        ("code".into(), "github-code".into()),
    ]);
    assert!(
        issuer
            .callback("github", &callback, "wrong-browser", &admission)
            .await
            .is_err()
    );
    let complete = issuer
        .callback("github", &callback, &browser, &admission)
        .await
        .unwrap();
    assert_eq!(admission.0.load(Ordering::SeqCst), 1);
    assert!(
        issuer
            .callback("github", &callback, &browser, &admission)
            .await
            .is_err()
    );
    let form = exchange(&complete);
    drop(issuer);
    let issuer = runtime(&path, &server.base);
    let tokens = issuer.token(&form).unwrap();
    assert_eq!(
        issuer
            .userinfo(tokens["access_token"].as_str().unwrap())
            .unwrap()["accountID"],
        "acc_fixture"
    );
    assert_eq!(issuer.discovery()["issuer"], "https://auth.opencode.ai");
    assert_eq!(issuer.jwks()["keys"][0]["kid"], "fixture-signing-key");
    assert!(issuer.jwks()["keys"][0].get("d").is_none());
    assert!(issuer.token(&form).is_err());
    let refresh = HashMap::from([
        ("grant_type".into(), "refresh_token".into()),
        (
            "refresh_token".into(),
            tokens["refresh_token"].as_str().unwrap().into(),
        ),
    ]);
    let next = issuer.token(&refresh).unwrap();
    assert_eq!(
        next["refresh_token"],
        issuer.token(&refresh).unwrap()["refresh_token"]
    );
    drop(issuer);
    let issuer = runtime(&path, &server.base);
    let successor = HashMap::from([
        ("grant_type".into(), "refresh_token".into()),
        (
            "refresh_token".into(),
            next["refresh_token"].as_str().unwrap().into(),
        ),
    ]);
    assert!(issuer.token(&successor).is_ok());
    let (subject, token) = tokens["refresh_token"]
        .as_str()
        .unwrap()
        .rsplit_once(':')
        .unwrap();
    assert!(
        issuer
            .store
            .refresh(&refresh_key(subject, token), now() + 61)
            .is_err()
    );
    server.stop().await;
}

#[tokio::test]
async fn google_oidc_signature_claims_state_and_nonce_acceptance() {
    let server = ProviderServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let issuer = runtime(&directory.path().join("issuer.sqlite"), &server.base);
    let admission = Admission(AtomicUsize::new(0));
    for invalid in [
        Some(("nonce", json!("attacker"))),
        Some(("iss", json!("https://attacker.example"))),
        Some(("aud", json!("other-client"))),
        Some(("email_verified", json!(false))),
        Some(("exp", json!(now() - 1))),
        Some(("azp", json!("other-client"))),
        None,
    ] {
        let start = issuer.authorize(&authorize("google", false)).await.unwrap();
        let (browser, provider) = flow(&start);
        assert_eq!(provider["response_type"], "id_token");
        assert_eq!(provider["response_mode"], "form_post");
        let id_token = google_token(&server.base, &provider["nonce"], invalid.clone());
        let values = HashMap::from([
            ("state".into(), provider["state"].clone()),
            ("id_token".into(), id_token),
        ]);
        let result = issuer
            .callback("google", &values, &browser, &admission)
            .await;
        if invalid.is_some() {
            assert!(result.unwrap().location.unwrap().contains("error="));
            continue;
        }
        let form = exchange(&result.unwrap());
        let tokens = issuer.token(&form).unwrap();
        assert_eq!(
            issuer
                .userinfo(tokens["access_token"].as_str().unwrap())
                .unwrap()["email"],
            "fixture@anoma.ly"
        );
        assert!(
            issuer
                .callback("google", &values, &browser, &admission)
                .await
                .is_err()
        );
    }
    assert_eq!(admission.0.load(Ordering::SeqCst), 1);
    let start = issuer.authorize(&authorize("google", false)).await.unwrap();
    let (browser, provider) = flow(&start);
    let mut token = google_token(&server.base, &provider["nonce"], None);
    let signature = token.rfind('.').unwrap() + 1;
    token.replace_range(
        signature..signature + 1,
        if &token[signature..signature + 1] == "A" {
            "B"
        } else {
            "A"
        },
    );
    assert!(
        issuer
            .callback(
                "google",
                &HashMap::from([
                    ("state".into(), provider["state"].clone()),
                    ("id_token".into(), token)
                ]),
                &browser,
                &admission
            )
            .await
            .unwrap()
            .location
            .unwrap()
            .contains("error=")
    );
    assert_eq!(admission.0.load(Ordering::SeqCst), 1);
    server.stop().await;
}

#[tokio::test]
async fn rejects_redirects_bad_pkce_and_provider_injection() {
    let server = ProviderServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let issuer = runtime(&directory.path().join("issuer.sqlite"), &server.base);
    for redirect in [
        "https://opencode.ai.evil.example/cb",
        "https://evil.example/cb",
        "javascript:alert(1)",
        "http://opencode.ai/cb",
    ] {
        let mut query = authorize("github", false);
        query.insert("redirect_uri".into(), redirect.into());
        assert!(matches!(
            issuer.authorize(&query).await,
            Err(Error::UnauthorizedClient)
        ));
    }
    let mut query = authorize("github", true);
    query.insert("code_challenge_method".into(), "plain".into());
    assert!(issuer.authorize(&query).await.is_err());
    assert!(issuer.authorize(&authorize("fake", false)).await.is_err());
    let admission = Admission(AtomicUsize::new(0));
    let mut existing = authorize("github", false);
    existing.insert(
        "redirect_uri".into(),
        "https://opencode.ai/auth/callback?code=old&state=old&keep=value".into(),
    );
    let start = issuer.authorize(&existing).await.unwrap();
    let (browser, provider) = flow(&start);
    let response = issuer
        .callback(
            "github",
            &HashMap::from([
                ("state".into(), provider["state"].clone()),
                ("code".into(), "github-code".into()),
            ]),
            &browser,
            &admission,
        )
        .await
        .unwrap();
    let destination = url::Url::parse(response.location.as_deref().unwrap()).unwrap();
    assert_eq!(
        destination
            .query_pairs()
            .filter(|(key, _)| key == "code")
            .count(),
        1
    );
    assert_eq!(
        destination
            .query_pairs()
            .filter(|(key, _)| key == "state")
            .count(),
        1
    );
    assert!(
        destination
            .query_pairs()
            .any(|(key, value)| key == "keep" && value == "value")
    );
    for field in ["code_verifier", "client_id", "redirect_uri"] {
        let start = issuer.authorize(&authorize("github", true)).await.unwrap();
        let (browser, provider) = flow(&start);
        let complete = issuer
            .callback(
                "github",
                &HashMap::from([
                    ("state".into(), provider["state"].clone()),
                    ("code".into(), "github-code".into()),
                ]),
                &browser,
                &admission,
            )
            .await
            .unwrap();
        let mut form = exchange(&complete);
        let original = form.clone();
        form.insert(field.into(), "wrong".into());
        assert!(issuer.token(&form).is_err());
        assert!(issuer.token(&original).is_err());
    }
    server.stop().await;
}

#[test]
fn durable_store_consumes_atomically_across_connections_and_expires() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("issuer.sqlite");
    let first = SqliteStore::open(&path).unwrap();
    let second = SqliteStore::open(&path).unwrap();
    first
        .put("once", &json!({"test":true}), now() + 60)
        .unwrap();
    assert_eq!(
        second.take("once", now()).unwrap(),
        Some(json!({"test":true}))
    );
    assert!(first.take("once", now()).unwrap().is_none());
    first
        .put("expired", &json!({"test":true}), now() - 1)
        .unwrap();
    assert!(second.take("expired", now()).unwrap().is_none());
    assert!(SqliteStore::open(std::path::Path::new(":memory:")).is_err());
}

#[test]
fn retired_key_ids_are_unique_and_never_expose_private_material() {
    let directory = tempfile::tempdir().unwrap();
    let issuer = runtime(
        &directory.path().join("original.sqlite"),
        "http://127.0.0.1:1",
    );
    let mut public = issuer.jwk.clone();
    public["kid"] = json!("retired-key");
    let mut config = issuer.config;
    config.retired_jwks = vec![public.clone(), public.clone()];
    assert!(
        IssuerRuntime::new(
            config,
            Arc::new(SqliteStore::open(&directory.path().join("duplicate.sqlite")).unwrap())
        )
        .is_err()
    );
    let issuer = runtime(
        &directory.path().join("second.sqlite"),
        "http://127.0.0.1:1",
    );
    let mut config = issuer.config;
    public["d"] = json!("private");
    config.retired_jwks = vec![public];
    assert!(
        IssuerRuntime::new(
            config,
            Arc::new(SqliteStore::open(&directory.path().join("private.sqlite")).unwrap())
        )
        .is_err()
    );
}
