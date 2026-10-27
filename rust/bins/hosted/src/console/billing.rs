//! Purpose: expose the Stripe webhook route and reconcile the supported top-up event.
//! Scope: public webhook verification and the baseline checkout credit fulfillment boundary.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- axum::{body::Bytes, Router}
//! |-- serde::Deserialize
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: exact request bytes are signature-checked before JSON decode or database access; no payment creation is performed.
//! Contract tests: hosted/src/console/billing/tests.rs; hosted-adapters/tests/console_commerce.rs

#![allow(clippy::result_large_err)]

use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use opencode_hosted_adapters::console::billing_store::{BillingStore, CheckoutCredit};
use opencode_hosted_adapters::console::stripe::{
    DEFAULT_API_BASE, StripeClient, StripeError, verify_webhook_signature,
};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use super::ConsoleState;

#[path = "billing/webhook.rs"]
mod lifecycle;
#[path = "billing/operations.rs"]
mod operations;
#[path = "billing/subscription.rs"]
mod subscription;

#[cfg(test)]
#[path = "billing/action_tests.rs"]
mod action_tests;
#[cfg(test)]
#[path = "billing/lifecycle_tests.rs"]
mod lifecycle_tests;
#[cfg(test)]
#[path = "billing/tests.rs"]
mod tests;

const SIGNATURE_HEADER: &str = "stripe-signature";

/// Config is module-owned so Stripe credentials never enter shared state or Debug output.
#[derive(Clone, Default)]
pub struct Config {
    pub secret_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub api_base: Option<String>,
    pub resources: std::collections::HashMap<String, String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("stripe_secret_configured", &self.secret_key.is_some())
            .field("webhook_secret_configured", &self.webhook_secret.is_some())
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let value = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        };
        let resources = [
            (
                "ZEN_LITE_PRICE",
                vec![
                    "product",
                    "firstMonth50Coupon",
                    "firstMonth100Coupon",
                    "threeMonths100Coupon",
                    "sixMonths100Coupon",
                    "twelveMonths100Coupon",
                ],
            ),
            (
                "ZEN_BLACK_PRICE",
                vec!["product", "plan20", "plan100", "plan200"],
            ),
        ]
        .into_iter()
        .flat_map(|(name, keys)| {
            keys.into_iter().filter_map(move |key| {
                lifecycle::resource(name, key).map(|value| (format!("{name}.{key}"), value))
            })
        })
        .collect();
        let config = Self {
            secret_key: value(&["OPENCODE_STRIPE_SECRET_KEY", "STRIPE_SECRET_KEY"]),
            webhook_secret: value(&["OPENCODE_STRIPE_WEBHOOK_SECRET", "STRIPE_WEBHOOK_SECRET"]),
            api_base: None,
            resources,
        };
        if let Some(api_base) = &config.api_base {
            let parsed =
                Url::parse(api_base).map_err(|_| anyhow::anyhow!("Invalid Stripe API base URL"))?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
                "Invalid Stripe API base URL"
            );
        }
        Ok(config)
    }
}

/// Public endpoint matching `packages/console/app/src/routes/stripe/webhook.ts`.
pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/stripe/webhook", post(webhook))
        .merge(operations::routes())
}

#[derive(Debug, Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(default)]
    created: i64,
    #[serde(rename = "type")]
    event_type: String,
    data: StripeEventData,
}

#[derive(Debug, Deserialize)]
struct StripeEventData {
    object: serde_json::Value,
    #[serde(default)]
    previous_attributes: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CheckoutSession {
    mode: Option<String>,
    metadata: Option<CheckoutMetadata>,
    customer: Option<StripeReference>,
    payment_intent: Option<StripeReference>,
    invoice: Option<StripeReference>,
}

#[derive(Debug, Deserialize)]
struct CheckoutMetadata {
    #[serde(rename = "workspaceID")]
    workspace_id: Option<String>,
    amount: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StripeReference {
    Id(String),
    Expanded { id: String },
}

impl StripeReference {
    fn id(&self) -> &str {
        match self {
            Self::Id(id) | Self::Expanded { id } => id,
        }
    }
}

async fn webhook(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let secret = state
        .config
        .billing
        .webhook_secret
        .as_deref()
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Stripe webhook is not configured",
            )
        })?;
    let signature = headers
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Missing Stripe signature"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    verify_webhook_signature(&body, signature, secret, now).map_err(signature_error)?;
    let event: StripeEvent = serde_json::from_slice(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid Stripe event"))?;
    if event.event_type != "checkout.session.completed" {
        let message = lifecycle::dispatch(&state, &event).await?;
        return Ok(Json(json!({"message":message})).into_response());
    }
    let object: CheckoutSession = serde_json::from_value(event.data.object)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid checkout session"))?;
    if object.mode.as_deref() != Some("payment") {
        return Ok(Json(json!({"message":"ignored"})).into_response());
    }
    let metadata = object
        .metadata
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Workspace ID not found"))?;
    let workspace_id = metadata
        .workspace_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Workspace ID not found"))?;
    let amount_cents = metadata
        .amount
        .and_then(|amount| amount.parse::<i64>().ok())
        .filter(|amount| *amount > 0)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Amount not found"))?;
    let customer_id = object
        .customer
        .as_ref()
        .map(StripeReference::id)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Customer ID not found"))?;
    let payment_id = object
        .payment_intent
        .as_ref()
        .map(StripeReference::id)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Payment ID not found"))?;
    let invoice_id = object
        .invoice
        .as_ref()
        .map(StripeReference::id)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Invoice ID not found"))?;
    let stripe_secret = state
        .config
        .billing
        .secret_key
        .as_deref()
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Stripe API is not configured",
            )
        })?;
    let stripe = StripeClient::new(
        stripe_secret,
        state
            .config
            .billing
            .api_base
            .as_deref()
            .unwrap_or(DEFAULT_API_BASE),
    )
    .map_err(stripe_error)?;
    let current = BillingStore::new(state.db.clone())
        .summary(&workspace_id)
        .await
        .map_err(store_error)?;
    if current
        .customer_id
        .as_deref()
        .is_some_and(|id| id != customer_id)
    {
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Customer ID mismatch",
        ));
    }
    if current.customer_id.is_none() {
        stripe
            .request(
                reqwest::Method::POST,
                "customers",
                Some(customer_id),
                None,
                &[("metadata[workspaceID]".into(), workspace_id.clone())],
            )
            .await
            .map_err(stripe_error)?;
    }
    let intent = stripe
        .payment_intent(payment_id)
        .await
        .map_err(stripe_error)?;
    let credit = CheckoutCredit {
        event_id: event.id,
        workspace_id,
        customer_id: customer_id.to_string(),
        payment_id: payment_id.to_string(),
        invoice_id: invoice_id.to_string(),
        amount_cents,
        payment_method_id: intent.payment_method.id,
        payment_method_type: intent.payment_method.method_type,
        payment_method_last4: intent.payment_method.card.and_then(|card| card.last4),
    };
    let outcome = BillingStore::new(state.db.clone())
        .complete_checkout(&credit)
        .await
        .map_err(store_error)?;
    let _ = outcome;
    Ok(Json(json!({"message":"done"})).into_response())
}

fn signature_error(failure: StripeError) -> Response {
    let status = match failure {
        StripeError::StaleSignature
        | StripeError::InvalidSignature
        | StripeError::SignatureHeader => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &failure.to_string())
}

fn stripe_error(failure: StripeError) -> Response {
    tracing::error!(error = %failure, "Stripe request failed");
    if let StripeError::Api { message, .. } = failure {
        return error(StatusCode::BAD_GATEWAY, &message);
    }
    error(StatusCode::BAD_GATEWAY, "Stripe request failed")
}

fn store_error(failure: impl std::fmt::Display) -> Response {
    tracing::error!(error = %failure, "billing store failure");
    error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

fn billing_error(
    failure: opencode_hosted_adapters::console::billing_store::BillingStoreError,
) -> Response {
    use opencode_hosted_domain::console::common::ConsoleError;
    match failure {
        opencode_hosted_adapters::console::billing_store::BillingStoreError::Console(failure) => {
            let status = match failure {
                ConsoleError::Validation(_) => StatusCode::BAD_REQUEST,
                ConsoleError::PermissionDenied(_) | ConsoleError::Actor(_) => StatusCode::FORBIDDEN,
                ConsoleError::NotFound(_) => StatusCode::NOT_FOUND,
                ConsoleError::Conflict(_) => StatusCode::CONFLICT,
                ConsoleError::NotYetPorted(_) => StatusCode::NOT_IMPLEMENTED,
            };
            error(status, &failure.to_string())
        }
        failure => store_error(failure),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"message":message}))).into_response()
}
