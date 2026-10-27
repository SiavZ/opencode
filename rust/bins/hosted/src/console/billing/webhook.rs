//! Purpose: dispatch baseline Stripe customer, subscription, invoice and refund webhooks.
//! Scope: signed event fulfillment through Stripe and durable billing adapters.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! Invariants: signature verification precedes dispatch; all money mutations are transactional and replay safe.
//! Contract tests: hosted/src/console/billing/lifecycle_tests.rs

use super::*;
use opencode_hosted_adapters::console::billing_store::{InvoicePayment, LiteSubscription};
use opencode_hosted_adapters::console::stripe::PaymentMethod;
use serde_json::{Value, json};

pub(super) fn stripe(state: &ConsoleState) -> Result<StripeClient, Response> {
    let config = &state.config.billing;
    StripeClient::new(
        config
            .secret_key
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Stripe API is not configured",
                )
            })?,
        config.api_base.as_deref().unwrap_or(DEFAULT_API_BASE),
    )
    .map_err(stripe_error)
}

pub(super) fn reference(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
}

pub(super) fn required<'a>(
    object: &'a Value,
    pointer: &str,
    message: &str,
) -> Result<&'a str, Response> {
    object
        .pointer(pointer)
        .and_then(reference)
        .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, message))
}

pub(super) fn resource(name: &str, key: &str) -> Option<String> {
    std::env::var(format!("OPENCODE_{name}_{}", key.to_ascii_uppercase()))
        .ok()
        .or_else(|| {
            std::env::var(format!("SST_RESOURCE_{name}"))
                .ok()
                .and_then(|value| serde_json::from_str::<Value>(&value).ok())
                .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_owned))
        })
}

pub(super) fn configured_resource(state: &ConsoleState, name: &str, key: &str) -> Option<String> {
    state
        .config
        .billing
        .resources
        .get(&format!("{name}.{key}"))
        .cloned()
}

pub(super) async fn dispatch(
    state: &ConsoleState,
    event: &StripeEvent,
) -> Result<&'static str, Response> {
    let object = &event.data.object;
    let store = BillingStore::new(state.db.clone());
    match event.event_type.as_str() {
        "customer.updated" => {
            if event
                .data
                .previous_attributes
                .pointer("/invoice_settings/default_payment_method")
                .is_none()
            {
                return Ok("ignored");
            }
            let customer = required(object, "/id", "Customer ID not found")?;
            let method = required(
                object,
                "/invoice_settings/default_payment_method",
                "Payment method ID not found",
            )?;
            let method: PaymentMethod = serde_json::from_value(
                stripe(state)?
                    .get_object("payment_methods", method, &[])
                    .await
                    .map_err(stripe_error)?,
            )
            .map_err(store_error)?;
            store
                .update_payment_method(customer, &method)
                .await
                .map_err(store_error)?;
        }
        "customer.subscription.created"
            if object.pointer("/metadata/type").and_then(Value::as_str) == Some("lite") =>
        {
            let workspace = required(object, "/metadata/workspaceID", "Workspace ID not found")?;
            let user = required(object, "/metadata/userID", "User ID not found")?;
            let customer = required(object, "/customer", "Customer ID not found")?;
            required(object, "/latest_invoice", "Invoice ID not found")?;
            let subscription = required(object, "/id", "Subscription ID not found")?;
            let method = required(
                object,
                "/default_payment_method",
                "Payment method ID not found",
            )?;
            let stripe = stripe(state)?;
            let method: PaymentMethod = serde_json::from_value(
                stripe
                    .get_object("payment_methods", method, &[])
                    .await
                    .map_err(stripe_error)?,
            )
            .map_err(store_error)?;
            let billing = store.summary(workspace).await.map_err(store_error)?;
            if billing
                .customer_id
                .as_deref()
                .is_some_and(|id| id != customer)
            {
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Customer ID mismatch",
                ));
            }
            if billing.customer_id.is_none() {
                stripe
                    .request(
                        reqwest::Method::POST,
                        "customers",
                        Some(customer),
                        None,
                        &[("metadata[workspaceID]".into(), workspace.into())],
                    )
                    .await
                    .map_err(stripe_error)?;
            }
            let coupon = object
                .pointer("/metadata/coupon")
                .and_then(Value::as_str)
                .and_then(|coupon| {
                    [
                        ("firstMonth50Coupon", "GO1MONTH50"),
                        ("firstMonth100Coupon", "GOFREEMONTH"),
                        ("threeMonths100Coupon", "GO3MONTHS100"),
                        ("sixMonths100Coupon", "GO6MONTHS100"),
                        ("twelveMonths100Coupon", "GO12MONTHS100"),
                    ]
                    .into_iter()
                    .find(|(key, _)| {
                        configured_resource(state, "ZEN_LITE_PRICE", key).as_deref() == Some(coupon)
                    })
                    .and_then(|(_, code)| {
                        object
                            .pointer("/metadata/userEmail")
                            .and_then(Value::as_str)
                            .map(|email| (email.into(), code.into()))
                    })
                });
            store
                .subscribe_lite(&LiteSubscription {
                    workspace_id: workspace.into(),
                    user_id: user.into(),
                    customer_id: customer.into(),
                    subscription_id: subscription.into(),
                    payment_method: method,
                    coupon,
                })
                .await
                .map_err(store_error)?;
            if let Err(failure) =
                opencode_hosted_adapters::console::referral::ReferralStore::new(state.db.clone())
                    .complete_from_lite_subscription(workspace, user)
                    .await
            {
                tracing::error!(error=%failure,"Referral sync failed");
            }
        }
        "customer.subscription.deleted" | "customer.subscription.updated" => {
            let subscription = required(object, "/id", "Subscription ID not found")?;
            let product = object
                .pointer("/items/data/0/price/product")
                .and_then(reference);
            let lite = product.is_some()
                && product == configured_resource(state, "ZEN_LITE_PRICE", "product").as_deref();
            let black = product.is_some()
                && product == configured_resource(state, "ZEN_BLACK_PRICE", "product").as_deref();
            let deleted = event.event_type == "customer.subscription.deleted";
            if (deleted || object["status"] == "incomplete_expired") && (lite || black) {
                store
                    .unsubscribe(subscription, lite)
                    .await
                    .map_err(store_error)?;
            }
            if deleted {
                if let Some(invoice) = object.get("latest_invoice").and_then(reference) {
                    let stripe = stripe(state)?;
                    let invoice_object = stripe
                        .get_object("invoices", invoice, &[])
                        .await
                        .map_err(stripe_error)?;
                    if invoice_object["status"] == "open" {
                        stripe
                            .request(
                                reqwest::Method::POST,
                                "invoices",
                                Some(invoice),
                                Some("void"),
                                &[],
                            )
                            .await
                            .map_err(stripe_error)?;
                    }
                }
            } else {
                if !black
                    || !matches!(
                        object["status"].as_str(),
                        Some("active" | "trialing" | "past_due")
                    )
                    || object["cancel_at_period_end"].as_bool() == Some(true)
                    || object
                        .get("cancel_at")
                        .is_some_and(|value| !value.is_null() && value.as_i64() != Some(0))
                {
                    return Ok("ignored");
                }
                stripe(state)?
                    .update_subscription(
                        subscription,
                        &[
                            ("cancel_at_period_end".into(), "true".into()),
                            (
                                "cancellation_details[comment]".into(),
                                "Legacy Black retirement: renewal is not allowed".into(),
                            ),
                        ],
                    )
                    .await
                    .map_err(stripe_error)?;
            }
        }
        "invoice.payment_succeeded" => {
            let reason = object["billing_reason"].as_str().unwrap_or_default();
            if !matches!(
                reason,
                "manual" | "subscription_create" | "subscription_cycle"
            ) {
                return Ok("done");
            }
            let customer = required(object, "/customer", "Customer ID not found")?;
            let invoice = required(object, "/id", "Invoice ID not found")?;
            let stripe = stripe(state)?;
            let expanded = stripe
                .get_object("invoices", invoice, &["discounts", "payments"])
                .await
                .map_err(stripe_error)?;
            let payment = expanded
                .pointer("/payments/data/0/payment/payment_intent")
                .and_then(reference)
                .map(str::to_owned);
            let coupon = expanded
                .pointer("/discounts/0/coupon/id")
                .and_then(Value::as_str);
            let workspace = if reason == "manual" {
                required(object, "/metadata/workspaceID", "Workspace ID not found")?.to_owned()
            } else {
                store
                    .customer_workspace(customer)
                    .await
                    .map_err(store_error)?
            };
            let amount = if reason == "manual" {
                object
                    .pointer("/metadata/amount")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .filter(|value| *value > 0)
            } else {
                object["amount_paid"].as_i64()
            }
            .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "Amount not found"))?;
            let enrichment = if reason == "manual" {
                None
            } else {
                required(
                    object,
                    "/parent/subscription_details/subscription",
                    "Subscription ID not found",
                )?;
                if payment.is_none() && coupon.is_none() {
                    return Err(error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Payment ID not found",
                    ));
                }
                let lite = object
                    .pointer("/lines/data/0/pricing/price_details/product")
                    .and_then(reference)
                    .is_some_and(|product| {
                        configured_resource(state, "ZEN_LITE_PRICE", "product").as_deref()
                            == Some(product)
                    });
                let mut value = json!({"type":if lite {"lite"} else {"subscription"}});
                if object["currency"] == "inr" {
                    value["currency"] = "inr".into();
                }
                if let Some(coupon) = coupon {
                    value["couponID"] = coupon.into();
                }
                Some(value)
            };
            store
                .record_invoice(&InvoicePayment {
                    workspace_id: workspace,
                    customer_id: customer.into(),
                    invoice_id: invoice.into(),
                    payment_id: payment,
                    amount_cents: amount,
                    enrichment,
                    credit_balance: reason == "manual",
                })
                .await
                .map_err(store_error)?;
        }
        "invoice.payment_failed" | "invoice.payment_action_required"
            if object["billing_reason"] == "manual" =>
        {
            let workspace = required(object, "/metadata/workspaceID", "Workspace ID not found")?;
            let invoice = required(object, "/id", "Invoice ID not found")?;
            let intent = stripe(state)?
                .get_object("payment_intents", invoice, &[])
                .await
                .map_err(stripe_error)?;
            store
                .set_reload_error(
                    workspace,
                    intent
                        .pointer("/last_payment_error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("workspace.reload.error.paymentFailed"),
                )
                .await
                .map_err(store_error)?;
        }
        "charge.refunded" => {
            store
                .refund(
                    required(object, "/customer", "Customer ID not found")?,
                    required(object, "/payment_intent", "Payment ID not found")?,
                    event.created,
                )
                .await
                .map_err(store_error)?;
        }
        _ => return Ok("ignored"),
    }
    Ok("done")
}
