//! Purpose: public HTTP issuer acceptance with local GitHub/Google, SQLite and MySQL admission.
//! Scope: isolated fixture services, no production providers or storage.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/issuer.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::serve
//! |-- mysql_async::prelude::Queryable
//! `-- reqwest::Client
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/issuer.rs
//! Invariants: account subject cannot be issued before invitation claim and Black check succeed.
//! Contract tests: this file

use super::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_domain::console::{common::Actor, identity::AuthProvider};
use serde_json::{Value, json};

mod sdk;

const KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgw7tDBuHtqIOB6FRR\nzJbG5AHmjP2dmnYddCN/3nE8RduhRANCAARPuKdPr3mreSJ9kiLkcz+02BWgFnA0\nsa2VMbNAn76sI5Wg4dQ2tB5AE9jAUwqZ1OnKSSstI9BcQODDGOngmrWI\n-----END PRIVATE KEY-----\n";

struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn providers() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app=Router::new()
            .route("/.well-known/openid-configuration",get(|State(base):State<String>| async move { Json(json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"jwks_uri":format!("{base}/jwks")})) }))
            .route("/jwks",get(|| async {
                let mut key=serde_json::to_value(jsonwebtoken::jwk::Jwk::from_encoding_key(&EncodingKey::from_ec_pem(KEY).unwrap(),Algorithm::ES256).unwrap()).unwrap();
                key["kid"]=json!("google-fixture"); key["alg"]=json!("ES256");
                Json(json!({"keys":[key]}))
            }))
            .route("/token",post(|Form(form):Form<HashMap<String,String>>| async move {
                assert_eq!(form["client_secret"],"fixture-secret");
                assert_eq!(form["grant_type"],"authorization_code");
                if form["code"]!="github-fixture-code" { return (StatusCode::BAD_REQUEST,Json(json!({"error":"invalid_grant"}))); }
                (StatusCode::OK,Json(json!({"access_token":"github-fixture-access"})))
            }))
            .route("/user",get(|headers:HeaderMap| async move {
                assert_eq!(headers["authorization"],"Bearer github-fixture-access");
                Json(json!({"id":424242}))
            }))
            .route("/user/emails",get(|headers:HeaderMap| async move {
                assert_eq!(headers["authorization"],"Bearer github-fixture-access");
                Json(json!([{"email":"issuer@anoma.ly","primary":true,"verified":true}]))
            })).with_state(base.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base, task }
    }

    async fn issuer(
        db: opencode_hosted_adapters::console::db::ConsoleDb,
        provider: &str,
        path: &std::path::Path,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let runtime = IssuerRuntime::new(
            IssuerConfig {
                issuer: base.clone(),
                signing_key_pem: KEY.to_vec(),
                key_id: "issuer-fixture".into(),
                retired_jwks: vec![],
                github: GithubConfig {
                    client_id: "fixture-github".into(),
                    client_secret: "fixture-secret".into(),
                    authorization_endpoint: format!("{provider}/github-authorize"),
                    token_endpoint: format!("{provider}/token"),
                    api_base: provider.into(),
                },
                google: GoogleConfig {
                    client_id: "fixture-google".into(),
                    issuer: provider.into(),
                },
            },
            Arc::new(SqliteStore::open(path).unwrap()),
        )
        .unwrap();
        let state = ConsoleState {
            db,
            config: Arc::new(super::super::state::ConsoleConfig {
                stage: "production".into(),
                issuer: Config {
                    runtime: Some(Arc::new(runtime)),
                },
                ..Default::default()
            }),
        };
        let task = tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        Self { base, task }
    }

    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn github_callback(http: &reqwest::Client, issuer: &str) -> reqwest::Response {
    let response = http
        .get(format!("{issuer}/authorize"))
        .query(&[
            ("client_id", "app"),
            ("redirect_uri", "http://localhost:3456/auth/callback"),
            ("response_type", "code"),
            ("state", "app-state"),
            ("provider", "github"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("HttpOnly")
    );
    let provider = url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: HashMap<String, String> = provider.query_pairs().into_owned().collect();
    http.get(format!("{issuer}/github/callback"))
        .header(header::COOKIE, cookie)
        .query(&[
            ("state", query["state"].as_str()),
            ("code", "github-fixture-code"),
        ])
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn public_issuer_claims_invites_checks_black_and_issues_verified_tokens() {
    let database = TestDatabase::create("issuer_http").await.unwrap();
    let store = TenantStore::new(database.db.clone());
    let provider = Server::providers().await;
    let directory = tempfile_for_issuer();
    let issuer = Server::issuer(database.db.clone(), &provider.base, &directory).await;
    let http = client();

    let unknown = github_callback(&http, &issuer.base).await;
    assert_eq!(unknown.status(), StatusCode::FOUND);
    assert_eq!(
        unknown.headers()[header::LOCATION],
        format!("{}/console", issuer.base)
    );
    assert!(
        store
            .auth_matches(AuthProvider::Github, "424242", "issuer@anoma.ly")
            .await
            .unwrap()
            .is_empty()
    );

    let account = store.create_account(None).await.unwrap();
    let mut connection = database.db.conn().await.unwrap();
    connection.exec_drop("INSERT INTO `auth` (`id`,`account_id`,`provider`,`subject`) VALUES ('aut_issuer_fixture',?,'email','issuer@anoma.ly')",(&account,)).await.unwrap();
    let not_black = github_callback(&http, &issuer.base).await;
    assert_eq!(
        not_black.headers()[header::LOCATION],
        format!("{}/console", issuer.base)
    );
    assert_eq!(
        store
            .auth_matches(AuthProvider::Github, "424242", "issuer@anoma.ly")
            .await
            .unwrap()
            .len(),
        1
    );

    let owner = store.create_account(None).await.unwrap();
    let workspace = store
        .create_workspace(
            &Actor::Account {
                account_id: owner,
                email: "owner@anoma.ly".into(),
            },
            "Issuer invited workspace",
        )
        .await
        .unwrap();
    connection
        .exec_drop(
            "UPDATE `billing` SET `subscription_id`='sub_issuer_fixture' WHERE `workspace_id`=?",
            (&workspace,),
        )
        .await
        .unwrap();
    connection.exec_drop("INSERT INTO `user` (`id`,`name`,`email`,`workspace_id`,`role`) VALUES ('usr_issuer_invited','','issuer@anoma.ly',?,'member')",(&workspace,)).await.unwrap();
    let complete = github_callback(&http, &issuer.base).await;
    assert_eq!(complete.status(), StatusCode::FOUND);
    assert_eq!(complete.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        complete.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let destination =
        url::Url::parse(complete.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: HashMap<String, String> = destination.query_pairs().into_owned().collect();
    assert_eq!(query["state"], "app-state");
    let claimed: Option<String> = connection
        .exec_first(
            "SELECT `account_id` FROM `user` WHERE `id`='usr_issuer_invited'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(claimed.as_deref(), Some(account.as_str()));
    assert_eq!(
        store
            .auth_matches(AuthProvider::Github, "424242", "issuer@anoma.ly")
            .await
            .unwrap()
            .len(),
        2
    );
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", "app"),
        ("redirect_uri", "http://localhost:3456/auth/callback"),
        ("code", query["code"].as_str()),
        ("code_verifier", ""),
    ];
    let response = http
        .post(format!("{}/token", issuer.base))
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    let tokens: Value = response.json().await.unwrap();
    let userinfo: Value = http
        .get(format!("{}/userinfo", issuer.base))
        .bearer_auth(tokens["access_token"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        userinfo,
        json!({"accountID":account,"email":"issuer@anoma.ly","newAccount":false})
    );
    assert_eq!(
        http.post(format!("{}/token", issuer.base))
            .form(&form)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let refreshed: Value = http
        .post(format!("{}/token", issuer.base))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens["refresh_token"].as_str().unwrap()),
        ])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(refreshed["refresh_token"], tokens["refresh_token"]);

    let discovery: Value = http
        .get(format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.base
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(discovery["issuer"], issuer.base);
    let keys: Value = http
        .get(discovery["jwks_uri"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(keys["keys"][0].get("d").is_none());
    let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(keys["keys"][0].clone()).unwrap();
    let mut validation = jsonwebtoken::Validation::new(Algorithm::ES256);
    validation.set_issuer(&[&issuer.base]);
    validation.set_audience(&["app"]);
    let claims = jsonwebtoken::decode::<Value>(
        tokens["access_token"].as_str().unwrap(),
        &jsonwebtoken::DecodingKey::from_jwk(&jwk).unwrap(),
        &validation,
    )
    .unwrap()
    .claims;
    assert_eq!(claims["properties"], userinfo);
    sdk::acceptance(&issuer.base, &account, directory.parent().unwrap()).await;

    // Google implicit OIDC posts a signed ID token, never accepts raw identity JSON.
    let response = http
        .get(format!("{}/authorize", issuer.base))
        .query(&[
            ("client_id", "app"),
            ("redirect_uri", "http://localhost:3456/auth/callback"),
            ("response_type", "code"),
            ("state", "google-app-state"),
            ("provider", "google"),
        ])
        .send()
        .await
        .unwrap();
    let browser = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let authorization =
        url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let params: HashMap<String, String> = authorization.query_pairs().into_owned().collect();
    let mut head = Header::new(Algorithm::ES256);
    head.kid = Some("google-fixture".into());
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let id=jsonwebtoken::encode(&head,&json!({"iss":provider.base,"aud":"fixture-google","sub":"google-fixture-sub","email":"issuer@anoma.ly","email_verified":true,"nonce":params["nonce"],"exp":now+60,"iat":now}),&EncodingKey::from_ec_pem(KEY).unwrap()).unwrap();
    let callback = http
        .post(format!("{}/google/callback", issuer.base))
        .header(header::COOKIE, browser.clone())
        .form(&[
            ("state", params["state"].as_str()),
            ("id_token", id.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::FOUND);
    assert!(
        callback.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .contains("code=")
    );
    assert_eq!(
        http.post(format!("{}/google/callback", issuer.base))
            .header(header::COOKIE, browser)
            .form(&[
                ("state", params["state"].as_str()),
                ("id_token", id.as_str())
            ])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.get(format!("{}/authorize", issuer.base))
            .query(&[
                ("client_id", "app"),
                ("response_type", "code"),
                ("redirect_uri", "https://evil.example/cb")
            ])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.request(reqwest::Method::OPTIONS, format!("{}/token", issuer.base))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    drop(connection);
    issuer.stop().await;
    provider.stop().await;
    std::fs::remove_file(directory).unwrap();
    database.drop_database().await.unwrap();
}

fn tempfile_for_issuer() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "opencode-issuer-{}.sqlite",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    assert!(!path.exists());
    path
}

#[tokio::test]
async fn issuer_unconfigured_is_closed_and_console_does_not_serve_issuer() {
    use tower::ServiceExt;
    let db = opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://fixture:fixture@127.0.0.1:1/fixture",
    )
    .unwrap();
    let state = ConsoleState {
        db,
        config: Arc::new(super::super::state::ConsoleConfig::default()),
    };
    let request = || {
        axum::http::Request::builder()
            .uri("/.well-known/jwks.json")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    assert_eq!(
        router(state.clone())
            .oneshot(request())
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        super::super::router(state)
            .oneshot(request())
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}
