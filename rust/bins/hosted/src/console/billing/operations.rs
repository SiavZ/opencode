//! Purpose: expose authenticated JSON transport for baseline billing server actions.
//! Scope: checkout, portal, receipt, settings, redemption and billing read operations.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/billing.rs
//! `-- hosted/src/console/billing/webhook.rs
//! rust/crates/
//! `-- hosted-domain/src/console/billing.rs
//! External:
//! |-- axum::{extract, routing}
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! Invariants: all actions resolve a workspace user; writes enforce same-origin before decoding.
//! Contract tests: hosted/src/console/billing/lifecycle_tests.rs

use super::super::auth;
use super::*;
use axum::extract::{OriginalUri, Path, Query};
use axum::http::Method;
use axum::routing::get;
use opencode_hosted_domain::console::billing::{calculate_fee_in_cents, checkout_amount};
use opencode_hosted_domain::console::common::Actor;
use serde_json::Value;
use std::collections::HashMap;

pub(super) fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/workspaces/{id}/billing", get(handle))
        .route("/workspaces/{id}/billing/payments", get(handle))
        .route("/workspaces/{id}/billing/usage", get(handle))
        .route("/workspaces/{id}/billing/checkout", post(handle))
        .route("/workspaces/{id}/billing/portal", post(handle))
        .route("/workspaces/{id}/billing/receipt", post(handle))
        .route(
            "/workspaces/{id}/billing/reload",
            post(handle).patch(handle),
        )
        .route("/workspaces/{id}/billing/monthly-limit", post(handle))
        .route("/workspaces/{id}/billing/redeem", post(handle))
        .route("/workspaces/{id}/billing/lite/checkout", post(handle))
        .route("/workspaces/{id}/billing/subscribe", post(handle))
        .route("/workspaces/{id}/billing/subscription", get(handle))
        .route(
            "/workspaces/{id}/billing/subscription/cancel-waitlist",
            post(handle),
        )
        .route("/workspaces/{id}/billing/subscription/enroll", post(handle))
        .route(
            "/workspaces/{id}/billing/subscription/use-balance",
            axum::routing::patch(handle),
        )
        .route("/workspaces/{id}/billing/lite", get(handle))
        .route("/workspaces/{id}/billing/lite/usage", get(handle))
        .route(
            "/workspaces/{id}/billing/lite/use-balance",
            axum::routing::patch(handle),
        )
}

async fn handle(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(workspace): Path<String>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, Response> {
    if method != Method::GET {
        auth::require_same_origin(&state, &headers)?;
    }
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&workspace), &method).await?;
    let Actor::User {
        user_id,
        workspace_id,
        ..
    } = &actor
    else {
        return Err(error(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    if workspace_id != &workspace {
        return Err(error(StatusCode::FORBIDDEN, "Workspace mismatch"));
    }
    if uri.path().ends_with("/checkout") {
        let counters = state.config.zen.checkout_counters().ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Checkout rate limiter is not configured",
            )
        })?;
        counters
            .checkout(actor.account().map_err(store_error)?)
            .await
            .map_err(|failure| match failure {
                opencode_hosted_adapters::console::limits::LimitError::Exceeded { .. } => error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many payment attempts. Please try again later.",
                ),
                _ => error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Checkout rate limiter unavailable",
                ),
            })?;
    }
    let input = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid JSON body"))?
    };
    let action = uri.path().split("/billing").nth(1).unwrap_or_default();
    let mut result = execute(&state, &workspace, user_id, action, &method, &query, &input).await?;
    if action == "/lite" && result.is_object() && result["region"].is_null() {
        let country = headers
            .get("cf-ipcountry")
            .and_then(|value| value.to_str().ok());
        result["region"] = serde_json::to_value(
            opencode_hosted_adapters::console::tenant::TenantStore::new(state.db.clone())
                .set_default_region(&actor, country)
                .await
                .map_err(store_error)?,
        )
        .map_err(store_error)?;
    }
    auth::with_session(&state, &session, Json(result).into_response())
}

fn field<'a>(input: &'a Value, key: &str) -> Result<&'a str, Response> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, &format!("{key} is required")))
}

pub(super) async fn execute(
    state: &ConsoleState,
    workspace: &str,
    user: &str,
    action: &str,
    method: &Method,
    query: &HashMap<String, String>,
    input: &Value,
) -> Result<Value, Response> {
    let store = BillingStore::new(state.db.clone());
    match action {
        "/subscription" => subscription::query(state, workspace, user, false, None).await,
        "/lite" => subscription::query(state, workspace, user, true, None).await,
        "/lite/usage" => {
            subscription::query(
                state,
                workspace,
                user,
                true,
                Some(query.get("window").map(String::as_str).unwrap_or("")),
            )
            .await
        }
        "/subscription/cancel-waitlist" => {
            store
                .cancel_waitlist(workspace)
                .await
                .map_err(billing_error)?;
            Ok(json!({}))
        }
        "/subscription/use-balance" | "/lite/use-balance" => {
            store
                .set_use_balance(
                    workspace,
                    action.starts_with("/lite"),
                    input["useBalance"]
                        .as_bool()
                        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "useBalance is required"))?,
                )
                .await
                .map_err(billing_error)?;
            Ok(json!({}))
        }
        "" => {
            let mut value =
                serde_json::to_value(store.summary(workspace).await.map_err(billing_error)?)
                    .map_err(store_error)?;
            value["reloadAmountMin"] = 10.into();
            value["reloadTriggerMin"] = 5.into();
            value.as_object_mut().unwrap().remove("workspaceID");
            Ok(value)
        }
        "/payments" => Ok(serde_json::to_value(
            store.payments(workspace).await.map_err(billing_error)?,
        )
        .map_err(store_error)?),
        "/usage" => Ok(serde_json::to_value(
            store
                .usages(
                    workspace,
                    query
                        .get("page")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0),
                    query
                        .get("limit")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(50),
                )
                .await
                .map_err(billing_error)?,
        )
        .map_err(store_error)?),
        "/monthly-limit" => {
            store
                .set_monthly_limit(
                    workspace,
                    input["limit"]
                        .as_i64()
                        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Invalid monthly limit"))?,
                )
                .await
                .map_err(billing_error)?;
            Ok(Value::Null)
        }
        "/reload" if method == Method::PATCH => {
            store
                .update_reload(
                    workspace,
                    input["reload"]
                        .as_bool()
                        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Invalid reload setting"))?,
                    input["reloadAmount"].as_i64(),
                    input["reloadTrigger"].as_i64(),
                )
                .await
                .map_err(billing_error)?;
            Ok(Value::Null)
        }
        "/reload" => {
            let billing = store.summary(workspace).await.map_err(billing_error)?;
            let customer = billing
                .customer_id
                .as_deref()
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No customer ID"))?;
            let payment = billing
                .payment_method_id
                .as_deref()
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No payment method"))?;
            if let Err(failure) = lifecycle::stripe(state)?
                .reload(customer, payment, workspace, billing.reload_amount)
                .await
            {
                store
                    .set_reload_error(workspace, &failure.to_string())
                    .await
                    .map_err(billing_error)?;
            }
            Ok(Value::Null)
        }
        "/redeem" => {
            let code = field(input, "code")?.trim().to_uppercase();
            if code.is_empty() {
                return Ok(json!({"error":"Coupon code is required."}));
            }
            if ![
                "BUILDATHON",
                "GO1MONTH50",
                "GOFREEMONTH",
                "GO3MONTHS100",
                "GO6MONTHS100",
                "GO12MONTHS100",
            ]
            .contains(&code.as_str())
            {
                return Ok(json!({"error":"Invalid coupon code."}));
            }
            let Some(email) = store
                .auth_email(workspace, user)
                .await
                .map_err(billing_error)?
            else {
                return Ok(json!({"error":"No email on account."}));
            };
            match store.redeem_coupon(workspace, &email, &code).await {
                Ok(()) => Ok(json!({"data":true})),
                Err(failure) => Ok(json!({"error":failure.to_string()})),
            }
        }
        "/checkout" => {
            let billing = store.summary(workspace).await.map_err(billing_error)?;
            let amount = checkout_amount(
                input
                    .get("amount")
                    .map(|value| {
                        value
                            .as_i64()
                            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Invalid amount"))
                    })
                    .transpose()?,
                Some(billing.reload_amount),
            )
            .map_err(|failure| error(StatusCode::BAD_REQUEST, &failure.to_string()))?
            .checked_mul(100)
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Amount outside supported range"))?;
            let mut fields = vec![
                ("mode".into(), "payment".into()),
                ("billing_address_collection".into(), "required".into()),
                ("currency".into(), "usd".into()),
                ("invoice_creation[enabled]".into(), "true".into()),
                (
                    "payment_method_options[card][setup_future_usage]".into(),
                    "off_session".into(),
                ),
                (
                    "payment_method_options[link][setup_future_usage]".into(),
                    "off_session".into(),
                ),
                ("tax_id_collection[enabled]".into(), "true".into()),
                ("metadata[workspaceID]".into(), workspace.into()),
                ("metadata[amount]".into(), amount.to_string()),
                ("success_url".into(), field(input, "successUrl")?.into()),
                ("cancel_url".into(), field(input, "cancelUrl")?.into()),
            ];
            for (index, (name, amount)) in [
                ("opencode credits", amount),
                ("processing fee", calculate_fee_in_cents(amount)),
            ]
            .into_iter()
            .enumerate()
            {
                fields.extend([
                    (
                        format!("line_items[{index}][price_data][currency]"),
                        "usd".into(),
                    ),
                    (
                        format!("line_items[{index}][price_data][product_data][name]"),
                        name.into(),
                    ),
                    (
                        format!("line_items[{index}][price_data][unit_amount]"),
                        amount.to_string(),
                    ),
                    (format!("line_items[{index}][quantity]"), "1".into()),
                ]);
            }
            if let Some(customer) = billing.customer_id {
                fields.extend([
                    ("customer".into(), customer),
                    ("customer_update[name]".into(), "auto".into()),
                    ("customer_update[address]".into(), "auto".into()),
                ]);
            } else {
                let email = store
                    .auth_email(workspace, user)
                    .await
                    .map_err(billing_error)?
                    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No email on account."))?;
                fields.extend([
                    ("customer_email".into(), email),
                    ("customer_creation".into(), "always".into()),
                ]);
            }
            Ok(lifecycle::stripe(state)?
                .create_object("checkout/sessions", &fields)
                .await
                .map_err(stripe_error)?["url"]
                .clone())
        }
        "/portal" => {
            let billing = store.summary(workspace).await.map_err(billing_error)?;
            let customer = billing
                .customer_id
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No stripe customer ID"))?;
            Ok(lifecycle::stripe(state)?
                .create_object(
                    "billing_portal/sessions",
                    &[
                        ("customer".into(), customer),
                        ("return_url".into(), field(input, "returnUrl")?.into()),
                    ],
                )
                .await
                .map_err(stripe_error)?["url"]
                .clone())
        }
        "/receipt" => {
            let payment = field(input, "paymentID")?;
            if !store
                .has_payment(workspace, payment)
                .await
                .map_err(billing_error)?
            {
                return Err(error(StatusCode::NOT_FOUND, "Payment not found"));
            }
            let stripe = lifecycle::stripe(state)?;
            let intent = stripe
                .get_object("payment_intents", payment, &[])
                .await
                .map_err(stripe_error)?;
            let charge = lifecycle::required(&intent, "/latest_charge", "No charge found")?;
            let charge = stripe
                .get_object("charges", charge, &[])
                .await
                .map_err(stripe_error)?;
            Ok(lifecycle::required(&charge, "/receipt_url", "No receipt URL found")?.into())
        }
        "/lite/checkout" => Err(error(
            StatusCode::BAD_REQUEST,
            "Go subscriptions have moved to the new Console",
        )),
        "/subscribe" | "/subscription/enroll" => {
            let seats = if action.ends_with("enroll") {
                1
            } else {
                input["seats"]
                    .as_i64()
                    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Seats required"))?
            };
            if input.get("coupon").is_some_and(|value| !value.is_string()) {
                return Err(error(StatusCode::BAD_REQUEST, "Invalid coupon"));
            }
            let billing = store.summary(workspace).await.map_err(billing_error)?;
            if billing.time_subscription_selected.is_none() {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "Not selected for subscription",
                ));
            }
            if billing.subscription_id.is_some() {
                return Err(error(StatusCode::BAD_REQUEST, "Already subscribed"));
            }
            let customer = billing
                .customer_id
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No customer ID"))?;
            let payment = billing
                .payment_method_id
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No payment method"))?;
            let plan = billing
                .subscription_plan
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "No subscription plan"))?;
            let price =
                lifecycle::configured_resource(state, "ZEN_BLACK_PRICE", &format!("plan{plan}"))
                    .ok_or_else(|| {
                        error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Subscription price is not configured",
                        )
                    })?;
            let subscription = lifecycle::stripe(state)?
                .create_object(
                    "subscriptions",
                    &[
                        ("customer".into(), customer),
                        ("default_payment_method".into(), payment),
                        ("items[0][price]".into(), price),
                        ("metadata[workspaceID]".into(), workspace.into()),
                    ],
                )
                .await
                .map_err(stripe_error)?;
            let id = lifecycle::required(&subscription, "/id", "Subscription ID not found")?;
            store
                .subscribe_black(workspace, user, id, seats, input["coupon"].as_str(), &plan)
                .await
                .map_err(billing_error)?;
            Ok(if action.ends_with("enroll") {
                json!({})
            } else {
                id.into()
            })
        }
        _ => Err(error(StatusCode::NOT_FOUND, "Billing operation not found")),
    }
}
