//! Purpose: reconcile durable invoice, refund and subscription lifecycle effects.
//! Scope: transactional MySQL billing and payment ledger updates.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing/webhook.rs
//! Invariants: row locks serialize credits and refunds; repeated invoice deliveries never double-credit.
//! Contract tests: hosted-adapters/tests/console_commerce_lifecycle.rs

use super::super::stripe::PaymentMethod;
use super::{BillingResult, BillingStore, CreditOutcome};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::{ConsoleError, IdKind, create_id};

pub struct InvoicePayment {
    pub workspace_id: String,
    pub customer_id: String,
    pub invoice_id: String,
    pub payment_id: Option<String>,
    pub amount_cents: i64,
    pub enrichment: Option<serde_json::Value>,
    pub credit_balance: bool,
}

pub struct LiteSubscription {
    pub workspace_id: String,
    pub user_id: String,
    pub customer_id: String,
    pub subscription_id: String,
    pub payment_method: PaymentMethod,
    pub coupon: Option<(String, String)>,
}

impl BillingStore {
    pub async fn record_invoice(&self, payment: &InvoicePayment) -> BillingResult<CreditOutcome> {
        if payment.invoice_id.is_empty()
            || payment.customer_id.is_empty()
            || payment.amount_cents < 0
            || (payment.credit_balance && payment.amount_cents == 0)
        {
            return Err(ConsoleError::Validation("Invalid invoice payment details".into()).into());
        }
        let amount = payment
            .amount_cents
            .checked_mul(1_000_000)
            .ok_or_else(|| ConsoleError::Validation("Amount outside supported range".into()))?;
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let row: Option<Option<String>> = tx
            .exec_first(
                "SELECT `customer_id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE",
                (&payment.workspace_id,),
            )
            .await?;
        let Some(customer) = row else {
            return Err(ConsoleError::NotFound("Workspace ID not found".into()).into());
        };
        if customer
            .as_deref()
            .is_some_and(|id| id != payment.customer_id)
        {
            return Err(ConsoleError::Conflict("Customer ID mismatch".into()).into());
        }
        let exists: Option<String> = tx.exec_first("SELECT `id` FROM `payment` WHERE `workspace_id` = ? AND (`invoice_id` = ? OR (`payment_id` = ? AND `payment_id` IS NOT NULL)) LIMIT 1", (&payment.workspace_id,&payment.invoice_id,&payment.payment_id)).await?;
        if exists.is_some() {
            tx.rollback().await?;
            return Ok(CreditOutcome::Duplicate);
        }
        if payment.credit_balance {
            tx.exec_drop("UPDATE `billing` SET `balance` = `balance` + ?, `reload_error` = NULL, `time_reload_error` = NULL WHERE `workspace_id` = ?", (amount,&payment.workspace_id)).await?;
        }
        tx.exec_drop("INSERT INTO `payment` (`id`,`workspace_id`,`customer_id`,`invoice_id`,`payment_id`,`amount`,`enrichment`) VALUES (?, ?, ?, ?, ?, ?, ?)", (create_id(IdKind::Payment,None)?,&payment.workspace_id,&payment.customer_id,&payment.invoice_id,&payment.payment_id,amount,payment.enrichment.as_ref().map(ToString::to_string))).await?;
        tx.commit().await?;
        Ok(CreditOutcome::Applied)
    }

    pub async fn refund(
        &self,
        customer: &str,
        payment: &str,
        created: i64,
    ) -> BillingResult<CreditOutcome> {
        let workspace = self.customer_workspace(customer).await?;
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let _: Option<String> = tx
            .exec_first(
                "SELECT `id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE",
                (&workspace,),
            )
            .await?;
        let row: Option<(i64,Option<String>,Option<time::PrimitiveDateTime>)> = tx.exec_first("SELECT `amount`,`enrichment`,`time_refunded` FROM `payment` WHERE `workspace_id` = ? AND `payment_id` = ? FOR UPDATE", (&workspace,payment)).await?;
        let Some((amount, enrichment, refunded)) = row else {
            return Err(ConsoleError::NotFound("Payment not found".into()).into());
        };
        if refunded.is_some() {
            tx.rollback().await?;
            return Ok(CreditOutcome::Duplicate);
        }
        let enrichment: Option<serde_json::Value> = enrichment
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        if enrichment
            .as_ref()
            .and_then(|value| value.get("type"))
            .is_none()
        {
            tx.exec_drop(
                "UPDATE `billing` SET `balance` = `balance` - ? WHERE `workspace_id` = ?",
                (amount, &workspace),
            )
            .await?;
        }
        tx.exec_drop("UPDATE `payment` SET `time_refunded` = FROM_UNIXTIME(?) WHERE `workspace_id` = ? AND `payment_id` = ?", (created,&workspace,payment)).await?;
        tx.commit().await?;
        Ok(CreditOutcome::Applied)
    }

    pub async fn subscribe_lite(&self, subscription: &LiteSubscription) -> BillingResult<()> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let row: Option<(Option<String>,Option<String>)> = tx.exec_first("SELECT `customer_id`,`lite_subscription_id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE", (&subscription.workspace_id,)).await?;
        let Some((customer, current)) = row else {
            return Err(ConsoleError::NotFound("Workspace ID not found".into()).into());
        };
        if customer
            .as_deref()
            .is_some_and(|id| id != subscription.customer_id)
        {
            return Err(ConsoleError::Conflict("Customer ID mismatch".into()).into());
        }
        if current.as_deref() == Some(&subscription.subscription_id) {
            tx.rollback().await?;
            return Ok(());
        }
        if let Some((email, code)) = &subscription.coupon {
            let coupon: Option<Option<time::PrimitiveDateTime>> = tx.exec_first("SELECT `time_redeemed` FROM `coupon` WHERE `email` = ? AND `type` = ? FOR UPDATE", (email,code)).await?;
            if code != "GO1MONTH50" {
                match coupon {
                    None => {
                        return Err(ConsoleError::Validation("Invalid coupon code".into()).into());
                    }
                    Some(Some(_)) => {
                        return Err(ConsoleError::Conflict("Coupon already redeemed".into()).into());
                    }
                    Some(None) => {}
                }
            }
            tx.exec_drop("INSERT INTO `coupon` (`email`,`type`,`time_redeemed`) VALUES (?, ?, NOW(3)) ON DUPLICATE KEY UPDATE `time_redeemed` = NOW(3)", (email,code)).await?;
        }
        let method = &subscription.payment_method;
        tx.exec_drop("UPDATE `billing` SET `customer_id` = ?, `lite_subscription_id` = ?, `lite` = JSON_OBJECT(), `payment_method_id` = ?, `payment_method_type` = ?, `payment_method_last4` = ? WHERE `workspace_id` = ?", (&subscription.customer_id,&subscription.subscription_id,&method.id,&method.method_type,method.card.as_ref().and_then(|card| card.last4.as_deref()),&subscription.workspace_id)).await?;
        tx.exec_drop("INSERT INTO `lite` (`id`,`workspace_id`,`user_id`) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE `time_deleted` = NULL", (create_id(IdKind::Lite,None)?,&subscription.workspace_id,&subscription.user_id)).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn unsubscribe(&self, subscription: &str, lite: bool) -> BillingResult<()> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let (column, table, json_column) = if lite {
            ("lite_subscription_id", "lite", "lite")
        } else {
            ("subscription_id", "subscription", "subscription")
        };
        let workspace: Option<String> = tx
            .exec_first(
                format!("SELECT `workspace_id` FROM `billing` WHERE `{column}` = ? FOR UPDATE"),
                (subscription,),
            )
            .await?;
        // Replayed deletes must acknowledge an already completed cleanup.
        if let Some(workspace) = workspace {
            tx.exec_drop(format!("UPDATE `billing` SET `{column}` = NULL, `{json_column}` = NULL WHERE `workspace_id` = ?"), (&workspace,)).await?;
            tx.exec_drop(
                format!("DELETE FROM `{table}` WHERE `workspace_id` = ?"),
                (&workspace,),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn subscribe_black(
        &self,
        workspace: &str,
        user: &str,
        subscription: &str,
        seats: i64,
        coupon: Option<&str>,
        plan: &str,
    ) -> BillingResult<()> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let current: Option<Option<String>> = tx
            .exec_first(
                "SELECT `subscription_id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE",
                (workspace,),
            )
            .await?;
        if current.flatten().is_some() {
            return Err(ConsoleError::Conflict("Already subscribed".into()).into());
        }
        let mut value = serde_json::json!({"status":"subscribed","seats":seats,"plan":plan});
        if let Some(coupon) = coupon {
            value["coupon"] = coupon.into();
        }
        tx.exec_drop("UPDATE `billing` SET `subscription_id` = ?, `subscription` = ?, `subscription_plan` = NULL, `time_subscription_booked` = NULL, `time_subscription_selected` = NULL WHERE `workspace_id` = ?", (subscription,value.to_string(),workspace)).await?;
        tx.exec_drop(
            "INSERT INTO `subscription` (`id`,`workspace_id`,`user_id`) VALUES (?, ?, ?)",
            (create_id(IdKind::Subscription, None)?, workspace, user),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
