//! Purpose: run baseline Black gift and transfer orchestration against explicit local Stripe capture.
//! Scope: local operator HTTP requests followed by durable MySQL transactions.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/stripe.rs
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! |-- reqwest::Method
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators.rs
//! Invariants: Stripe endpoint must be literal loopback; no production credentials or inferred payment destinations.
//! Contract tests: hosted-adapters/tests/console_workers_operators.rs

use super::field;
use crate::console::{db::ConsoleDb, stripe::StripeClient, workers::WorkerError};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::{IdKind, create_id};
use reqwest::Method;
use serde_json::{Value, json};

fn client(input: &Value) -> Result<StripeClient, WorkerError> {
    let endpoint = field(input, "stripeCaptureUrl")?;
    let url = url::Url::parse(endpoint).map_err(|e| WorkerError::Validation(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.host_str().is_some_and(|host| {
            host.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
    {
        return Err(WorkerError::Validation(
            "stripeCaptureUrl must use a literal loopback IP".into(),
        ));
    }
    StripeClient::new("fixture-operator", endpoint)
        .map_err(|e| WorkerError::Operation(e.to_string()))
}

pub(super) async fn gift(input: &Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    let stripe = client(input)?;
    let workspace = field(input, "workspaceID")?;
    let seats = input
        .get("seats")
        .and_then(Value::as_i64)
        .filter(|n| *n != 0)
        .ok_or_else(|| WorkerError::Validation("seats is required".into()))?;
    let price = field(input, "priceID")?;
    let mut conn = db.conn().await?;
    let users:Vec<(String,String,Option<String>)>=conn.exec("SELECT u.id,u.role,a.subject FROM `user` u LEFT JOIN auth a ON a.account_id = u.account_id AND a.provider = 'email' WHERE u.workspace_id = ? AND u.time_deleted IS NULL",(workspace,)).await?;
    if users.is_empty() {
        return Err(WorkerError::Validation(format!(
            "No users found in workspace {workspace}"
        )));
    }
    if users.len() as i64 != seats {
        return Err(WorkerError::Validation(format!(
            "Workspace {workspace} has {} users, expected {seats}",
            users.len()
        )));
    }
    let admin = users.iter().find(|row| row.1 == "admin").ok_or_else(|| {
        WorkerError::Validation(format!("No admin user found in workspace {workspace}"))
    })?;
    let email = admin
        .2
        .as_ref()
        .ok_or_else(|| WorkerError::Validation(format!("Admin user {} has no email", admin.0)))?;
    let billing: Option<(Option<String>, Option<String>)> = conn
        .exec_first(
            "SELECT customer_id,subscription_id FROM billing WHERE workspace_id = ?",
            (workspace,),
        )
        .await?;
    let (customer, subscription) = billing.ok_or_else(|| {
        WorkerError::Validation(format!("Workspace {workspace} has no billing record"))
    })?;
    if subscription.is_some() {
        return Err(WorkerError::Validation(format!(
            "Workspace {workspace} already has a subscription"
        )));
    }
    let customer = match customer {
        Some(customer) => customer,
        None => {
            let value = stripe
                .request(
                    Method::POST,
                    "customers",
                    None,
                    None,
                    &[
                        ("email".into(), email.clone()),
                        ("metadata[workspaceID]".into(), workspace.into()),
                    ],
                )
                .await
                .map_err(|e| WorkerError::Operation(e.to_string()))?;
            field(&value, "id")?.to_string()
        }
    };
    let subscription = stripe
        .request(
            Method::POST,
            "subscriptions",
            None,
            None,
            &[
                ("customer".into(), customer.clone()),
                ("items[0][price]".into(), price.into()),
                ("items[0][discounts][0][coupon]".into(), "JAIr0Pe1".into()),
                ("items[0][quantity]".into(), seats.to_string()),
                ("metadata[workspaceID]".into(), workspace.into()),
            ],
        )
        .await
        .map_err(|e| WorkerError::Operation(e.to_string()))?;
    let subscription = field(&subscription, "id")?;
    drop(conn);
    let mut tx = db.pool().start_transaction(TxOpts::default()).await?;
    tx.exec_drop("UPDATE billing SET customer_id = ?, subscription_id = ?, subscription = ? WHERE workspace_id = ?",(&customer,subscription,json!({"status":"subscribed","coupon":"JAIr0Pe1","seats":seats,"plan":"200"}).to_string(),workspace)).await?;
    for user in users {
        let id = create_id(IdKind::Subscription, None)
            .map_err(|e| WorkerError::Operation(e.to_string()))?;
        tx.exec_drop(
            "INSERT INTO subscription (id,workspace_id,user_id) VALUES (?, ?, ?)",
            (id, workspace, user.0),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(json!({"workspaceID":workspace,"customerID":customer,"subscriptionID":subscription}))
}

pub(super) async fn transfer(input: &Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    let stripe = client(input)?;
    let from = field(input, "fromWorkspaceID")?;
    let to = field(input, "toWorkspaceID")?;
    let mut conn = db.conn().await?;
    type TransferBillingRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row:Option<TransferBillingRow>=conn.exec_first("SELECT customer_id,subscription_id,CAST(subscription AS CHAR),payment_method_id,payment_method_type,payment_method_last4 FROM billing WHERE workspace_id = ?",(from,)).await?;
    let row =
        row.ok_or_else(|| WorkerError::Validation("FROM workspace has no billing record".into()))?;
    let customer = row.0.as_deref().ok_or_else(|| {
        WorkerError::Validation("FROM workspace has no Stripe customer ID".into())
    })?;
    let subscription = row
        .1
        .as_deref()
        .ok_or_else(|| WorkerError::Validation("FROM workspace has no subscription".into()))?;
    let user: Option<String> = conn
        .exec_first(
            "SELECT user_id FROM subscription WHERE workspace_id = ? LIMIT 1",
            (from,),
        )
        .await?;
    let user =
        user.ok_or_else(|| WorkerError::Validation("FROM workspace has no subscription".into()))?;
    let payments:Vec<(Option<String>,time::PrimitiveDateTime)>=conn.exec("SELECT customer_id,time_created FROM payment WHERE workspace_id = ? AND JSON_EXTRACT(enrichment,'$.type') = 'subscription'",(from,)).await?;
    if payments.len() > 1 {
        return Err(WorkerError::Validation(format!(
            "Multiple subscription payments found for workspace {from}"
        )));
    }
    let payment = payments.first().ok_or_else(|| {
        WorkerError::Validation("FROM workspace has no subscription payment".into())
    })?;
    let previous:Option<String>=conn.exec_first("SELECT customer_id FROM payment WHERE workspace_id = ? AND customer_id IS NOT NULL AND time_created < ? ORDER BY time_created DESC LIMIT 1",(from,payment.1)).await?;
    let previous = previous.ok_or_else(|| {
        WorkerError::Validation(
            "FROM workspace has no previous Stripe customer to revert to".into(),
        )
    })?;
    if previous == customer {
        return Err(WorkerError::Validation(
            "FROM workspace has the same Stripe customer ID as the current one".into(),
        ));
    }
    let methods = stripe
        .request(
            Method::GET,
            "customers",
            Some(&previous),
            Some("payment_methods"),
            &[],
        )
        .await
        .map_err(|e| WorkerError::Operation(e.to_string()))?;
    let method = methods
        .get("data")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .ok_or_else(|| {
            WorkerError::Validation("FROM workspace has no previous Stripe payment methods".into())
        })?;
    let target: Option<Option<String>> = conn
        .exec_first(
            "SELECT subscription_id FROM billing WHERE workspace_id = ?",
            (to,),
        )
        .await?;
    let target = target
        .ok_or_else(|| WorkerError::Validation("TO workspace has no billing record".into()))?;
    if target.is_some() {
        return Err(WorkerError::Validation(
            "TO workspace already has a subscription".into(),
        ));
    }
    for (id, workspace) in [(&previous[..], from), (customer, to)] {
        stripe
            .request(
                Method::POST,
                "customers",
                Some(id),
                None,
                &[("metadata[workspaceID]".into(), workspace.into())],
            )
            .await
            .map_err(|e| WorkerError::Operation(e.to_string()))?;
    }
    drop(conn);
    let mut tx = db.pool().start_transaction(TxOpts::default()).await?;
    // Clear source first, matching the baseline and freeing global customer/subscription indexes.
    tx.exec_drop("UPDATE billing SET customer_id = ?,subscription_id = NULL,subscription = NULL,payment_method_id = ?,payment_method_last4 = ?,payment_method_type = ? WHERE workspace_id = ?",(&previous,field(method,"id")?,method.pointer("/card/last4").and_then(Value::as_str),field(method,"type")?,from)).await?;
    tx.exec_drop("UPDATE billing SET customer_id = ?,subscription_id = ?,subscription = ?,payment_method_id = ?,payment_method_last4 = ?,payment_method_type = ? WHERE workspace_id = ?",(customer,subscription,row.2.as_deref(),row.3.as_deref(),row.5.as_deref(),row.4.as_deref(),to)).await?;
    tx.exec_drop(
        "UPDATE subscription SET workspace_id = ?, user_id = ? WHERE workspace_id = ?",
        (to, user, from),
    )
    .await?;
    tx.exec_drop("UPDATE payment SET workspace_id = ? WHERE workspace_id = ? AND JSON_EXTRACT(enrichment,'$.type') = 'subscription' AND amount = 20000000000",(to,from)).await?;
    tx.commit().await?;
    Ok(
        json!({"fromWorkspaceID":from,"toWorkspaceID":to,"customerID":customer,"subscriptionID":subscription}),
    )
}
