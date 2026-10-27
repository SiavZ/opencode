//! Purpose: persist console billing views and reconcile Stripe top-up completion atomically.
//! Scope: local MySQL repository over baseline billing, payment, and usage tables.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-domain/src/console/billing.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! |-- serde_json::Value
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! Invariants: workspace row lock serializes webhook effects; payment row and balance update commit together; existing payment or invoice ID is a durable duplicate key.
//! Contract tests: hosted-adapters/tests/console_commerce.rs

use super::db::{ConsoleDb, DbError};
use super::tenant::iso_opt;
use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::billing::{
    BillingSummary, PAYMENTS_PAGE_SIZE, Payment, USAGES_PAGE_SIZE, Usage,
};
use opencode_hosted_domain::console::common::{ConsoleError, IdKind, ulid};
use sha2::Digest;

#[path = "billing_store/lifecycle.rs"]
mod lifecycle;
#[path = "billing_store/operations.rs"]
mod operations;
pub use lifecycle::{InvoicePayment, LiteSubscription};
#[path = "billing_store/subscription.rs"]
mod subscription;
pub use subscription::SubscriptionUsage;

#[derive(Debug, thiserror::Error)]
pub enum BillingStoreError {
    #[error(transparent)]
    Console(#[from] ConsoleError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("mysql: {0}")]
    Mysql(#[from] mysql_async::Error),
    #[error("database returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type BillingResult<T> = Result<T, BillingStoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutCredit {
    pub event_id: String,
    pub workspace_id: String,
    pub customer_id: String,
    pub payment_id: String,
    pub invoice_id: String,
    pub amount_cents: i64,
    pub payment_method_id: String,
    pub payment_method_type: String,
    pub payment_method_last4: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BillingStore {
    db: ConsoleDb,
}

impl BillingStore {
    pub fn new(db: ConsoleDb) -> Self {
        Self { db }
    }

    pub async fn summary(&self, workspace_id: &str) -> BillingResult<BillingSummary> {
        let mut conn = self.db.conn().await?;
        let row: Option<mysql_async::Row> = conn
            .exec_first(
                "SELECT `customer_id`, `payment_method_id`, `payment_method_type`, `payment_method_last4`, `balance`, `monthly_limit`, `monthly_usage`, `time_monthly_usage_updated`, `reload`, `reload_trigger`, `reload_amount`, `reload_error`, `time_reload_error`, `subscription`, `subscription_id`, `time_subscription_booked`, `time_subscription_selected`, `lite`, `lite_subscription_id`, `subscription_plan` FROM `billing` WHERE `workspace_id` = ?",
                (workspace_id,),
            )
            .await?;
        let row = row.ok_or_else(|| ConsoleError::NotFound("Billing record not found".into()))?;
        let mut row = row;
        let time = |row: &mut mysql_async::Row, index| {
            iso_opt(
                row.take::<Option<time::PrimitiveDateTime>, _>(index)
                    .flatten(),
            )
        };
        Ok(BillingSummary {
            workspace_id: workspace_id.into(),
            customer_id: row.take(0).flatten(),
            payment_method_id: row.take(1).flatten(),
            payment_method_type: row.take(2).flatten(),
            payment_method_last4: row.take(3).flatten(),
            balance: row.take(4).unwrap_or_default(),
            monthly_limit: row.take(5).flatten(),
            monthly_usage: row.take(6).flatten(),
            time_monthly_usage_updated: time(&mut row, 7),
            reload: row.take(8).flatten(),
            reload_trigger: row.take::<Option<i64>, _>(9).flatten().unwrap_or(5),
            reload_amount: row.take::<Option<i64>, _>(10).flatten().unwrap_or(20),
            reload_error: row.take(11).flatten(),
            time_reload_error: time(&mut row, 12),
            subscription: row
                .take::<Option<String>, _>(13)
                .flatten()
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            subscription_id: row.take(14).flatten(),
            time_subscription_booked: time(&mut row, 15),
            time_subscription_selected: time(&mut row, 16),
            lite: row
                .take::<Option<String>, _>(17)
                .flatten()
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            lite_subscription_id: row.take(18).flatten(),
            subscription_plan: row.take::<Option<String>, _>(19).flatten(),
        })
    }

    pub async fn payments(&self, workspace_id: &str) -> BillingResult<Vec<Payment>> {
        let mut conn = self.db.conn().await?;
        let rows: Vec<mysql_async::Row> = conn
            .exec(
                "SELECT `id`, `time_created`, `customer_id`, `invoice_id`, `payment_id`, `amount`, `time_refunded`, `enrichment` FROM `payment` WHERE `workspace_id` = ? ORDER BY `time_created` DESC LIMIT ?",
                (workspace_id, PAYMENTS_PAGE_SIZE),
            )
            .await?;
        rows.into_iter()
            .map(|mut row| {
                Ok(Payment {
                    id: row.take(0).unwrap_or_default(),
                    time_created: iso(row.take(1).unwrap_or(time::PrimitiveDateTime::MIN)),
                    customer_id: row.take(2).flatten(),
                    invoice_id: row.take(3).flatten(),
                    payment_id: row.take(4).flatten(),
                    amount: row.take(5).unwrap_or_default(),
                    time_refunded: iso_opt(
                        row.take::<Option<time::PrimitiveDateTime>, _>(6).flatten(),
                    ),
                    enrichment: row
                        .take::<Option<String>, _>(7)
                        .flatten()
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                })
            })
            .collect()
    }

    pub async fn usages(
        &self,
        workspace_id: &str,
        page: u32,
        page_size: u32,
    ) -> BillingResult<Vec<Usage>> {
        let page_size = page_size.clamp(1, 100).min(USAGES_PAGE_SIZE);
        let offset = u64::from(page.saturating_mul(page_size));
        let mut conn = self.db.conn().await?;
        let rows: Vec<mysql_async::Row> = conn
            .exec(
                "SELECT `id`, `time_created`, `model`, `provider`, `input_tokens`, `output_tokens`, `reasoning_tokens`, `cache_read_tokens`, `cache_write_5m_tokens`, `cache_write_1h_tokens`, `cost`, `key_id`, `session_id`, `enrichment` FROM `usage` WHERE `workspace_id` = ? ORDER BY `time_created` DESC LIMIT ? OFFSET ?",
                (workspace_id, page_size, offset),
            )
            .await?;
        rows.into_iter()
            .map(|mut row| {
                Ok(Usage {
                    id: row.take(0).unwrap_or_default(),
                    time_created: iso(row.take(1).unwrap_or(time::PrimitiveDateTime::MIN)),
                    model: row.take(2).unwrap_or_default(),
                    provider: row.take(3).unwrap_or_default(),
                    input_tokens: row.take(4).unwrap_or_default(),
                    output_tokens: row.take(5).unwrap_or_default(),
                    reasoning_tokens: row.take(6).flatten(),
                    cache_read_tokens: row.take(7).flatten(),
                    cache_write_5m_tokens: row.take(8).flatten(),
                    cache_write_1h_tokens: row.take(9).flatten(),
                    cost: row.take(10).unwrap_or_default(),
                    key_id: row.take(11).flatten(),
                    session_id: row.take(12).flatten(),
                    enrichment: row
                        .take::<Option<String>, _>(13)
                        .flatten()
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                })
            })
            .collect()
    }

    /// Applies the baseline checkout credit once. Provider object IDs are persisted in `payment`
    /// and the billing row lock prevents concurrent deliveries racing the duplicate check.
    pub async fn complete_checkout(&self, credit: &CheckoutCredit) -> BillingResult<CreditOutcome> {
        if credit.amount_cents <= 0
            || credit.event_id.is_empty()
            || credit.payment_id.is_empty()
            || credit.invoice_id.is_empty()
        {
            return Err(ConsoleError::Validation("Invalid checkout payment details".into()).into());
        }
        let amount = credit
            .amount_cents
            .checked_mul(1_000_000)
            .ok_or_else(|| ConsoleError::Validation("Amount outside supported range".into()))?;
        let digest = sha2::Sha256::digest(credit.event_id.as_bytes());
        let millis = u64::from_be_bytes([
            0, 0, digest[0], digest[1], digest[2], digest[3], digest[4], digest[5],
        ]);
        let random = u128::from_be_bytes([
            0, 0, 0, 0, 0, 0, digest[6], digest[7], digest[8], digest[9], digest[10], digest[11],
            digest[12], digest[13], digest[14], digest[15],
        ]);
        let payment_record_id = format!("{}_{}", IdKind::Payment.prefix(), ulid(millis, random));
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let billing: Option<(i64, Option<String>)> = tx
            .exec_first(
                "SELECT `balance`, `customer_id` FROM `billing` WHERE `workspace_id` = ? FOR UPDATE",
                (&credit.workspace_id,),
            )
            .await?;
        let Some((_, customer_id)) = billing else {
            tx.rollback().await?;
            return Err(ConsoleError::NotFound("Workspace ID not found".into()).into());
        };
        if customer_id
            .as_deref()
            .is_some_and(|customer| customer != credit.customer_id)
        {
            tx.rollback().await?;
            return Err(ConsoleError::Conflict("Customer ID mismatch".into()).into());
        }
        let duplicate: Option<String> = tx
            .exec_first(
                "SELECT `id` FROM `payment` WHERE `workspace_id` = ? AND (`id` = ? OR `payment_id` = ? OR `invoice_id` = ?) LIMIT 1",
                (
                    &credit.workspace_id,
                    &payment_record_id,
                    &credit.payment_id,
                    &credit.invoice_id,
                ),
            )
            .await?;
        if duplicate.is_some() {
            tx.rollback().await?;
            return Ok(CreditOutcome::Duplicate);
        }
        tx.exec_drop(
            "UPDATE `billing` SET `balance` = `balance` + ?, `reload_error` = IF(`customer_id` IS NULL, NULL, `reload_error`), `time_reload_error` = IF(`customer_id` IS NULL, NULL, `time_reload_error`), `customer_id` = ?, `payment_method_id` = ?, `payment_method_type` = ?, `payment_method_last4` = ? WHERE `workspace_id` = ?",
            (
                amount,
                &credit.customer_id,
                &credit.payment_method_id,
                &credit.payment_method_type,
                &credit.payment_method_last4,
                &credit.workspace_id,
            ),
        )
        .await?;
        tx.exec_drop(
            "INSERT INTO `payment` (`id`, `workspace_id`, `amount`, `payment_id`, `invoice_id`, `customer_id`, `enrichment`) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                payment_record_id,
                &credit.workspace_id,
                amount,
                &credit.payment_id,
                &credit.invoice_id,
                &credit.customer_id,
                Option::<String>::None,
            ),
        )
        .await?;
        tx.commit().await?;
        Ok(CreditOutcome::Applied)
    }
}

fn iso(value: time::PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond()
    )
}
