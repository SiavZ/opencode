//! Purpose: fixture tests for OIDC verification: issuer, audience, expiry, signature, kid
//! lookup, kid rotation refetch, algorithm confusion and repository claim parsing.
//! Scope: test (fixture-only, local axum JWKS server, test RSA keys).
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/oidc.rs
//! `-- ecosystem/tests/github_auth_support/mod.rs
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::http::StatusCode::SERVICE_UNAVAILABLE
//! |-- axum::routing::get
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! |-- jsonwebtoken::Algorithm::HS256
//! |-- jsonwebtoken::Algorithm::RS256
//! |-- jsonwebtoken::EncodingKey::from_rsa_pem
//! |-- jsonwebtoken::EncodingKey::from_secret
//! |-- jsonwebtoken::Header::new
//! |-- jsonwebtoken::encode
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: no network beyond 127.0.0.1.
//! Contract tests: this file.

mod github_auth_support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{Json, Router, routing::get};
use github_auth_support::{
    APP_KEY_PEM, AUDIENCE, ISSUER, OTHER_KEY_PEM, good_claims, jwk, jwks, mint, now, serve,
};
use opencode_ecosystem::github::auth::oidc::{OidcVerifier, parse_repository_claim};
use serde_json::{Value, json};

async fn verifier_with(keys: Value) -> OidcVerifier {
    let router = Router::new().route("/jwks", get(move || async move { Json(keys) }));
    OidcVerifier::new(format!("{}/jwks", serve(router).await), ISSUER, AUDIENCE)
}

async fn key_set() -> OidcVerifier {
    verifier_with(jwks(vec![jwk(APP_KEY_PEM, "k1")])).await
}

#[tokio::test]
async fn fixture_valid_token_yields_repository() {
    let v = key_set().await;
    let repo = v
        .verify_repository(&mint(APP_KEY_PEM, Some("k1"), good_claims()))
        .await
        .unwrap();
    assert_eq!(
        (repo.owner.as_str(), repo.repo.as_str()),
        ("acme", "widgets")
    );
}

#[tokio::test]
async fn fixture_rejects_wrong_issuer_audience_expiry_and_signature() {
    let v = key_set().await;
    let mut wrong_iss = good_claims();
    wrong_iss["iss"] = json!("https://evil.example");
    let mut wrong_aud = good_claims();
    wrong_aud["aud"] = json!("someone-else");
    let mut expired = good_claims();
    expired["exp"] = json!(now() - 5);
    let mut no_aud = good_claims();
    no_aud.as_object_mut().unwrap().remove("aud");
    let mut future_nbf = good_claims();
    future_nbf["nbf"] = json!(now() + 600);
    let mut array_iss = good_claims();
    array_iss["iss"] = json!([ISSUER]);
    for claims in [wrong_iss, wrong_aud, expired, no_aud, future_nbf, array_iss] {
        let token = mint(APP_KEY_PEM, Some("k1"), claims.clone());
        assert!(v.verify(&token).await.is_err(), "accepted {claims}");
    }
    // Signed by a key that is not in the set but claims kid k1.
    let forged = mint(OTHER_KEY_PEM, Some("k1"), good_claims());
    assert!(v.verify(&forged).await.is_err());
}

#[tokio::test]
async fn fixture_audience_array_containing_expected_is_accepted() {
    let v = key_set().await;
    let mut claims = good_claims();
    claims["aud"] = json!(["other", AUDIENCE]);
    assert!(
        v.verify(&mint(APP_KEY_PEM, Some("k1"), claims))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn fixture_rejects_hmac_and_none_algorithms() {
    let v = key_set().await;
    let hs = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &good_claims(),
        &jsonwebtoken::EncodingKey::from_secret(b"secret"),
    )
    .unwrap();
    assert!(v.verify(&hs).await.is_err());
    // alg=none token: header {"alg":"none"} with an empty signature.
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let none = format!(
        "{}.{}.",
        b64.encode(r#"{"alg":"none","typ":"JWT"}"#),
        b64.encode(good_claims().to_string())
    );
    assert!(v.verify(&none).await.is_err());
}

#[tokio::test]
async fn fixture_unknown_kid_is_rejected() {
    let v = key_set().await;
    let token = mint(APP_KEY_PEM, Some("missing"), good_claims());
    let err = v.verify(&token).await.unwrap_err().to_string();
    assert!(err.contains("no applicable key"), "{err}");
}

#[tokio::test]
async fn fixture_token_without_kid_needs_single_candidate() {
    let single = key_set().await;
    assert!(
        single
            .verify(&mint(APP_KEY_PEM, None, good_claims()))
            .await
            .is_ok()
    );
    let multi = verifier_with(jwks(vec![jwk(APP_KEY_PEM, "a"), jwk(OTHER_KEY_PEM, "b")])).await;
    let err = multi
        .verify(&mint(APP_KEY_PEM, None, good_claims()))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("multiple matching keys"), "{err}");
}

#[tokio::test]
async fn fixture_kid_rotation_refetches_jwks() {
    let current = Arc::new(Mutex::new(jwks(vec![jwk(APP_KEY_PEM, "old")])));
    let fetches = Arc::new(Mutex::new(0));
    let (c, f) = (current.clone(), fetches.clone());
    let router = Router::new().route(
        "/jwks",
        get(move || {
            let (c, f) = (c.clone(), f.clone());
            async move {
                *f.lock().unwrap() += 1;
                Json(c.lock().unwrap().clone())
            }
        }),
    );
    let v = OidcVerifier::new(format!("{}/jwks", serve(router).await), ISSUER, AUDIENCE)
        .with_refetch_cooldown(Duration::ZERO);
    v.verify(&mint(APP_KEY_PEM, Some("old"), good_claims()))
        .await
        .unwrap();
    v.verify(&mint(APP_KEY_PEM, Some("old"), good_claims()))
        .await
        .unwrap();
    assert_eq!(*fetches.lock().unwrap(), 1, "cached key set reused");
    *current.lock().unwrap() = jwks(vec![jwk(OTHER_KEY_PEM, "new")]);
    v.verify(&mint(OTHER_KEY_PEM, Some("new"), good_claims()))
        .await
        .unwrap();
    assert_eq!(*fetches.lock().unwrap(), 2, "unknown kid refetched");
}

#[tokio::test]
async fn fixture_jwks_http_failure_is_error() {
    let router = Router::new().route(
        "/jwks",
        get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "down") }),
    );
    let v = OidcVerifier::new(format!("{}/jwks", serve(router).await), ISSUER, AUDIENCE);
    assert!(
        v.verify(&mint(APP_KEY_PEM, Some("k1"), good_claims()))
            .await
            .is_err()
    );
}

#[test]
fn fixture_repository_claim_parsing_matches_baseline() {
    let parse =
        |v: Value| parse_repository_claim(v.as_object().unwrap()).map_err(|e| e.to_string());
    assert_eq!(parse(json!({})).unwrap_err(), "Repository claim is missing");
    assert_eq!(
        parse(json!({"repository": 5})).unwrap_err(),
        "Repository claim is missing"
    );
    for bad in ["acme", "acme/", "/w", "a/b/c", ""] {
        assert_eq!(
            parse(json!({ "repository": bad })).unwrap_err(),
            "Repository claim is invalid",
            "{bad}"
        );
    }
    let ok = parse(json!({"repository": "o/r"})).unwrap();
    assert_eq!((ok.owner.as_str(), ok.repo.as_str()), ("o", "r"));
}

#[test]
fn fixture_github_defaults() {
    let debug = format!("{:?}", OidcVerifier::github());
    assert!(debug.contains("https://token.actions.githubusercontent.com/.well-known/jwks"));
    assert!(debug.contains("opencode-github-action"));
}

#[tokio::test]
async fn fixture_temporal_claim_types_and_optional_expiry() {
    let v = key_set().await;
    for field in ["exp", "nbf", "iat"] {
        for invalid in [json!("123"), Value::Null, json!(true)] {
            let mut claims = good_claims();
            claims[field] = invalid;
            assert!(
                v.verify(&mint(APP_KEY_PEM, Some("k1"), claims))
                    .await
                    .is_err(),
                "accepted malformed {field}"
            );
        }
    }
    let mut claims = good_claims();
    claims.as_object_mut().unwrap().remove("exp");
    claims["nbf"] = json!(-1.5);
    assert!(
        v.verify(&mint(APP_KEY_PEM, Some("k1"), claims))
            .await
            .is_ok()
    );
    let mut claims = good_claims();
    claims["exp"] = json!(now());
    assert!(
        v.verify(&mint(APP_KEY_PEM, Some("k1"), claims))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn fixture_critical_headers_reject_unknown_and_unencoded_payloads() {
    let v = key_set().await;
    for crit in [vec![], vec!["unknown"], vec!["b64"]] {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("k1".into());
        header.crit = Some(crit.iter().map(|s| s.to_string()).collect());
        header.extras.insert("b64", false);
        let token = jsonwebtoken::encode(
            &header,
            &good_claims(),
            &jsonwebtoken::EncodingKey::from_rsa_pem(APP_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(v.verify(&token).await.is_err());
    }
}

#[tokio::test]
async fn fixture_jwk_key_operations_must_allow_verification() {
    let mut keys = jwks(vec![jwk(APP_KEY_PEM, "k1")]);
    keys["keys"][0]["key_ops"] = json!(["sign"]);
    let v = verifier_with(keys).await;
    assert!(
        v.verify(&mint(APP_KEY_PEM, Some("k1"), good_claims()))
            .await
            .is_err()
    );
}
