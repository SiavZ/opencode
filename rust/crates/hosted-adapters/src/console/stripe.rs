//! Purpose: issue controlled Stripe API reads and verify Stripe webhook signatures on raw bytes.
//! Scope: HTTP/cryptographic boundary; no database or payment initiation.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/mod.rs
//! External:
//! |-- hmac::{Hmac, Mac}
//! |-- reqwest::Client
//! |-- serde::{Deserialize, Serialize}
//! |-- sha2::Sha256
//! `-- url::Url
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! Invariants: signatures cover exact raw bytes; timestamp tolerance is 300 seconds; configured API base is required to be HTTP(S).
//! Contract tests: hosted-adapters/tests/console_commerce_stripe.rs

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::Url;

#[path = "stripe/operations.rs"]
mod operations;

type HmacSha256 = Hmac<Sha256>;

pub const WEBHOOK_TOLERANCE_SECONDS: i64 = 300;
pub const DEFAULT_API_BASE: &str = "https://api.stripe.com";

#[derive(Debug, thiserror::Error)]
pub enum StripeError {
    #[error("invalid Stripe webhook signature header")]
    SignatureHeader,
    #[error("Stripe webhook timestamp outside tolerance")]
    StaleSignature,
    #[error("invalid Stripe webhook signature")]
    InvalidSignature,
    #[error("invalid Stripe API base URL")]
    ApiBase,
    #[error("Stripe API request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Stripe API returned HTTP {0}")]
    Status(reqwest::StatusCode),
    #[error("{message}")]
    Api {
        status: reqwest::StatusCode,
        message: String,
    },
    #[error("invalid Stripe API response: {0}")]
    Decode(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub method_type: String,
    pub card: Option<CardDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardDetails {
    pub last4: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentIntent {
    pub id: String,
    pub payment_method: PaymentMethod,
}

#[derive(Clone)]
pub struct StripeClient {
    secret_key: String,
    base: Url,
    client: reqwest::Client,
}

impl std::fmt::Debug for StripeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeClient")
            .field("secret_configured", &!self.secret_key.is_empty())
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl StripeClient {
    pub fn new(secret_key: impl Into<String>, api_base: &str) -> Result<Self, StripeError> {
        let base = Url::parse(api_base).map_err(|_| StripeError::ApiBase)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(StripeError::ApiBase);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            secret_key: secret_key.into(),
            base,
            client,
        })
    }

    pub async fn payment_intent(&self, id: &str) -> Result<PaymentIntent, StripeError> {
        Ok(serde_json::from_value(
            self.get_object("payment_intents", id, &["payment_method"])
                .await?,
        )?)
    }
}

/// Verifies the official `t=...,v1=...` scheme. A signature passes when any v1 entry matches.
pub fn verify_webhook_signature(
    payload: &[u8],
    header: &str,
    secret: &str,
    now_unix_seconds: i64,
) -> Result<(), StripeError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" if timestamp.is_none() => {
                timestamp = value.parse::<i64>().ok();
            }
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(StripeError::SignatureHeader)?;
    if signatures.is_empty() {
        return Err(StripeError::SignatureHeader);
    }
    let age = now_unix_seconds.saturating_sub(timestamp).unsigned_abs();
    if age > WEBHOOK_TOLERANCE_SECONDS as u64 {
        return Err(StripeError::StaleSignature);
    }
    for signature in signatures {
        let Ok(signature) = hex::decode(signature) else {
            continue;
        };
        let Ok(mut candidate) =
            <HmacSha256 as hmac::digest::KeyInit>::new_from_slice(secret.as_bytes())
        else {
            continue;
        };
        candidate.update(timestamp.to_string().as_bytes());
        candidate.update(b".");
        candidate.update(payload);
        if candidate.verify_slice(&signature).is_ok() {
            return Ok(());
        }
    }
    Err(StripeError::InvalidSignature)
}

pub fn sign_webhook_payload(payload: &[u8], timestamp: i64, secret: &str) -> String {
    let mut mac = <HmacSha256 as hmac::digest::KeyInit>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    format!(
        "t={timestamp},v1={}",
        hex::encode(mac.finalize().into_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_checks_raw_payload_and_timestamp_tolerance() {
        let raw = br#"{ "id":"evt_1" }"#;
        let signed = sign_webhook_payload(raw, 1_700_000_000, "whsec_fixture");
        assert!(verify_webhook_signature(raw, &signed, "whsec_fixture", 1_700_000_300).is_ok());
        assert!(matches!(
            verify_webhook_signature(raw, &signed, "whsec_fixture", 1_700_000_301),
            Err(StripeError::StaleSignature)
        ));
        assert!(matches!(
            verify_webhook_signature(
                br#"{"id":"evt_1"}"#,
                &signed,
                "whsec_fixture",
                1_700_000_000
            ),
            Err(StripeError::InvalidSignature)
        ));
        assert!(matches!(
            verify_webhook_signature(raw, "t=1700000000", "whsec_fixture", 1_700_000_000),
            Err(StripeError::SignatureHeader)
        ));
    }
}
