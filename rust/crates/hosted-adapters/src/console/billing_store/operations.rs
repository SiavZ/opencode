//! Purpose: persist editable billing settings and single-use coupon redemption.
//! Scope: local MySQL billing operations and actor email lookup.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! `-- mysql_async::{TxOpts, prelude::Queryable}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing/operations.rs
//! Invariants: BUILDATHON credit and coupon consumption commit atomically under a row lock.
//! Contract tests: hosted-adapters/tests/console_commerce_lifecycle.rs

use super::{BillingResult, BillingStore};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::{ConsoleError, IdKind, create_id};

impl BillingStore {
    pub async fn has_payment(&self, workspace: &str, payment: &str) -> BillingResult<bool> {
        let mut conn = self.db.conn().await?;
        let found: Option<String> = conn
            .exec_first(
                "SELECT id FROM payment WHERE workspace_id=? AND payment_id=? LIMIT 1",
                (workspace, payment),
            )
            .await?;
        Ok(found.is_some())
    }
    pub async fn customer_workspace(&self, customer: &str) -> BillingResult<String> {
        let mut conn = self.db.conn().await?;
        conn.exec_first(
            "SELECT `workspace_id` FROM `billing` WHERE `customer_id` = ?",
            (customer,),
        )
        .await?
        .ok_or_else(|| ConsoleError::NotFound("Workspace ID not found for customer".into()).into())
    }

    pub async fn auth_email(&self, workspace: &str, user: &str) -> BillingResult<Option<String>> {
        let mut conn = self.db.conn().await?;
        Ok(conn.exec_first("SELECT a.`subject` FROM `user` u JOIN `auth` a ON a.`account_id` = u.`account_id` AND a.`provider` = 'email' WHERE u.`workspace_id` = ? AND u.`id` = ? AND u.`time_deleted` IS NULL LIMIT 1", (workspace,user)).await?)
    }

    pub async fn set_monthly_limit(&self, workspace: &str, limit: i64) -> BillingResult<()> {
        opencode_hosted_domain::console::billing::validate_monthly_limit(limit)?;
        let mut conn = self.db.conn().await?;
        conn.exec_drop(
            "UPDATE `billing` SET `monthly_limit` = ? WHERE `workspace_id` = ?",
            (limit, workspace),
        )
        .await?;
        Ok(())
    }

    pub async fn update_reload(
        &self,
        workspace: &str,
        enabled: bool,
        amount: Option<i64>,
        trigger: Option<i64>,
    ) -> BillingResult<()> {
        opencode_hosted_domain::console::billing::validate_reload_settings(
            enabled, amount, trigger,
        )?;
        let mut conn = self.db.conn().await?;
        conn.exec_drop("UPDATE `billing` SET `reload` = ?, `reload_amount` = COALESCE(?, `reload_amount`), `reload_trigger` = COALESCE(?, `reload_trigger`), `reload_error` = IF(?, NULL, `reload_error`), `time_reload_error` = IF(?, NULL, `time_reload_error`) WHERE `workspace_id` = ?", (enabled,amount,trigger,enabled,enabled,workspace)).await?;
        Ok(())
    }

    pub async fn set_reload_error(&self, workspace: &str, message: &str) -> BillingResult<()> {
        let mut conn = self.db.conn().await?;
        conn.exec_drop("UPDATE `billing` SET `reload` = FALSE, `reload_error` = ?, `time_reload_error` = NOW(3) WHERE `workspace_id` = ?", (message,workspace)).await?;
        Ok(())
    }

    pub async fn update_payment_method(
        &self,
        customer: &str,
        method: &super::super::stripe::PaymentMethod,
    ) -> BillingResult<()> {
        let mut conn = self.db.conn().await?;
        conn.exec_drop("UPDATE `billing` SET `payment_method_id` = ?, `payment_method_type` = ?, `payment_method_last4` = ? WHERE `customer_id` = ?", (&method.id,&method.method_type,method.card.as_ref().and_then(|card| card.last4.as_deref()),customer)).await?;
        Ok(())
    }

    pub async fn redeem_coupon(
        &self,
        workspace: &str,
        email: &str,
        code: &str,
    ) -> BillingResult<()> {
        if ![
            "BUILDATHON",
            "GO1MONTH50",
            "GOFREEMONTH",
            "GO3MONTHS100",
            "GO6MONTHS100",
            "GO12MONTHS100",
        ]
        .contains(&code)
        {
            return Err(ConsoleError::Validation("Invalid coupon code".into()).into());
        }
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let coupon: Option<Option<time::PrimitiveDateTime>> = tx
            .exec_first(
                "SELECT `time_redeemed` FROM `coupon` WHERE `email` = ? AND `type` = ? FOR UPDATE",
                (email, code),
            )
            .await?;
        if code != "GO1MONTH50" {
            match coupon {
                None => return Err(ConsoleError::Validation("Invalid coupon code".into()).into()),
                Some(Some(_)) => {
                    return Err(ConsoleError::Conflict("Coupon already redeemed".into()).into());
                }
                Some(None) => {}
            }
        }
        if code == "BUILDATHON" {
            let exists: Option<String> = tx
                .exec_first(
                    "SELECT `id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE",
                    (workspace,),
                )
                .await?;
            if exists.is_none() {
                return Err(ConsoleError::NotFound("Billing record not found".into()).into());
            }
            tx.exec_drop(
                "UPDATE `billing` SET `balance` = `balance` + 50000000000 WHERE `workspace_id` = ?",
                (workspace,),
            )
            .await?;
            tx.exec_drop("INSERT INTO `payment` (`id`,`workspace_id`,`amount`,`enrichment`) VALUES (?, ?, 50000000000, JSON_OBJECT('type','credit'))", (create_id(IdKind::Payment,None)?,workspace)).await?;
        }
        tx.exec_drop("INSERT INTO `coupon` (`email`,`type`,`time_redeemed`) VALUES (?, ?, NOW(3)) ON DUPLICATE KEY UPDATE `time_redeemed` = NOW(3)", (email,code)).await?;
        tx.commit().await?;
        Ok(())
    }
}
