//! Purpose: shared fixture helpers for GitHub auth tests: test-only RSA keys, JWK sets,
//! token minting and a local axum server. Fixture-only.
//! Scope: test support module included with `mod`.
//! Uses:
//! `-- none
//! External:
//! |-- axum::Router
//! `-- jsonwebtoken::{encode, EncodingKey, Header, jwk::Jwk}
//! Used by:
//! rust/crates/
//! |-- ecosystem/tests/github_auth_client.rs
//! |-- ecosystem/tests/github_auth_exchange.rs
//! `-- ecosystem/tests/github_auth_oidc.rs
//! Invariants: keys come from tests/fixtures/auth (TEST KEYS ONLY).
//! Contract tests: n/a (helper).
#![allow(dead_code)]

use axum::Router;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};

pub const APP_KEY_PEM: &str = include_str!("../fixtures/auth/test-app-key.pem");
pub const OTHER_KEY_PEM: &str = include_str!("../fixtures/auth/test-other-key.pem");
pub const ISSUER: &str = "https://token.actions.githubusercontent.com";
pub const AUDIENCE: &str = "opencode-github-action";

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn jwk(pem: &str, kid: &str) -> Jwk {
    let key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some(kid.into());
    jwk
}

pub fn jwks(keys: Vec<Jwk>) -> Value {
    serde_json::to_value(JwkSet { keys }).unwrap()
}

pub fn mint(pem: &str, kid: Option<&str>, claims: Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_string);
    jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

pub fn good_claims() -> Value {
    json!({
        "iss": ISSUER,
        "aud": AUDIENCE,
        "exp": now() + 300,
        "iat": now(),
        "repository": "acme/widgets",
    })
}

pub async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}
