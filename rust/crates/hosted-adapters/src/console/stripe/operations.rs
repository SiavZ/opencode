//! Purpose: issue Stripe billing lifecycle requests using the baseline API version.
//! Scope: controlled HTTP adapter for checkout, portal, invoices and subscriptions.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- reqwest::Method
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! Invariants: dynamic identifiers are single URL path segments; requests never follow redirects.
//! Contract tests: hosted-adapters/tests/console_commerce_stripe.rs

use super::{StripeClient, StripeError};
use reqwest::Method;
use serde_json::Value;

impl StripeClient {
    pub async fn reload(
        &self,
        customer: &str,
        payment_method: &str,
        workspace: &str,
        amount_dollars: i64,
    ) -> Result<(), StripeError> {
        let amount = amount_dollars
            .checked_mul(100)
            .ok_or(StripeError::ApiBase)?;
        let draft = self
            .create_object(
                "invoices",
                &[
                    ("customer".into(), customer.into()),
                    ("auto_advance".into(), "false".into()),
                    ("default_payment_method".into(), payment_method.into()),
                    ("collection_method".into(), "charge_automatically".into()),
                    ("currency".into(), "usd".into()),
                    ("metadata[workspaceID]".into(), workspace.into()),
                    ("metadata[amount]".into(), amount.to_string()),
                ],
            )
            .await?;
        let invoice = draft["id"].as_str().ok_or(StripeError::ApiBase)?;
        for (amount, description) in [
            (amount, "opencode credits"),
            (
                opencode_hosted_domain::console::billing::calculate_fee_in_cents(amount),
                "processing fee",
            ),
        ] {
            self.create_object(
                "invoiceitems",
                &[
                    ("amount".into(), amount.to_string()),
                    ("currency".into(), "usd".into()),
                    ("customer".into(), customer.into()),
                    ("invoice".into(), invoice.into()),
                    ("description".into(), description.into()),
                ],
            )
            .await?;
        }
        self.request(
            Method::POST,
            "invoices",
            Some(invoice),
            Some("finalize"),
            &[],
        )
        .await?;
        self.request(
            Method::POST,
            "invoices",
            Some(invoice),
            Some("pay"),
            &[
                ("off_session".into(), "true".into()),
                ("payment_method".into(), payment_method.into()),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn request(
        &self,
        method: Method,
        resource: &str,
        id: Option<&str>,
        action: Option<&str>,
        fields: &[(String, String)],
    ) -> Result<Value, StripeError> {
        let mut url = self.base.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| StripeError::ApiBase)?;
            path.clear().push("v1");
            for segment in resource.split('/') {
                path.push(segment);
            }
            if let Some(id) = id {
                path.push(id);
            }
            if let Some(action) = action {
                path.push(action);
            }
        }
        if method == Method::GET && !fields.is_empty() {
            url.query_pairs_mut().extend_pairs(fields);
        }
        let request = self
            .client
            .request(method.clone(), url)
            .bearer_auth(&self.secret_key)
            .header("Stripe-Version", "2025-03-31.basil");
        let request = if method == Method::GET {
            request
        } else {
            request
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                )
        };
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if let Some(message) = body.pointer("/error/message").and_then(Value::as_str) {
                return Err(StripeError::Api {
                    status,
                    message: message.into(),
                });
            }
            return Err(StripeError::Status(status));
        }
        Ok(response.json().await?)
    }

    pub async fn get_object(
        &self,
        resource: &str,
        id: &str,
        expand: &[&str],
    ) -> Result<Value, StripeError> {
        self.request(
            Method::GET,
            resource,
            Some(id),
            None,
            &expand
                .iter()
                .map(|value| ("expand[]".into(), (*value).into()))
                .collect::<Vec<_>>(),
        )
        .await
    }

    pub async fn create_object(
        &self,
        resource: &str,
        fields: &[(String, String)],
    ) -> Result<Value, StripeError> {
        self.request(Method::POST, resource, None, None, fields)
            .await
    }

    pub async fn update_subscription(
        &self,
        id: &str,
        fields: &[(String, String)],
    ) -> Result<Value, StripeError> {
        self.request(Method::POST, "subscriptions", Some(id), None, fields)
            .await
    }

    pub async fn cancel_subscription(&self, id: &str) -> Result<Value, StripeError> {
        self.request(Method::DELETE, "subscriptions", Some(id), None, &[])
            .await
    }
}
