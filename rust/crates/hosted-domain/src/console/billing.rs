//! Purpose: model baseline workspace billing views, top-up rules, and processing fees.
//! Scope: pure console billing decisions; no Stripe calls or persistence.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- rust_decimal::Decimal
//! |-- serde::{Deserialize, Serialize}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: money persisted as integer microcents; checkout amounts are whole USD and never below baseline minimum.
//! Contract tests: hosted-domain/src/console/billing.rs

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::common::{ConsoleError, MicroCents};

pub const ITEM_CREDIT_NAME: &str = "opencode credits";
pub const ITEM_FEE_NAME: &str = "processing fee";
pub const RELOAD_AMOUNT: i64 = 20;
pub const RELOAD_AMOUNT_MIN: i64 = 10;
pub const RELOAD_TRIGGER: i64 = 5;
pub const RELOAD_TRIGGER_MIN: i64 = 5;
pub const PAYMENTS_PAGE_SIZE: u32 = 100;
pub const USAGES_PAGE_SIZE: u32 = 50;

/// Billing row projected with the baseline UI's defaults applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingSummary {
    #[serde(rename = "workspaceID")]
    pub workspace_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: Option<String>,
    #[serde(rename = "paymentMethodID")]
    pub payment_method_id: Option<String>,
    pub payment_method_type: Option<String>,
    pub payment_method_last4: Option<String>,
    pub balance: MicroCents,
    pub monthly_limit: Option<i64>,
    pub monthly_usage: Option<MicroCents>,
    pub time_monthly_usage_updated: Option<String>,
    pub reload: Option<bool>,
    pub reload_trigger: i64,
    pub reload_amount: i64,
    pub reload_error: Option<String>,
    pub time_reload_error: Option<String>,
    pub subscription: Option<serde_json::Value>,
    #[serde(rename = "subscriptionID")]
    pub subscription_id: Option<String>,
    pub subscription_plan: Option<String>,
    pub time_subscription_booked: Option<String>,
    pub time_subscription_selected: Option<String>,
    pub lite: Option<serde_json::Value>,
    #[serde(rename = "liteSubscriptionID")]
    pub lite_subscription_id: Option<String>,
}

/// Recorded payment returned by the baseline `Billing.payments()` server function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub id: String,
    pub time_created: String,
    #[serde(rename = "customerID")]
    pub customer_id: Option<String>,
    #[serde(rename = "invoiceID")]
    pub invoice_id: Option<String>,
    #[serde(rename = "paymentID")]
    pub payment_id: Option<String>,
    pub amount: MicroCents,
    pub time_refunded: Option<String>,
    pub enrichment: Option<serde_json::Value>,
}

/// Usage row returned by the baseline `Billing.usages()` server function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub id: String,
    pub time_created: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_5m_tokens: Option<i64>,
    pub cache_write_1h_tokens: Option<i64>,
    pub cost: MicroCents,
    #[serde(rename = "keyID")]
    pub key_id: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    pub enrichment: Option<serde_json::Value>,
}

/// Validates an optional checkout amount in whole USD and returns the selected amount.
pub fn checkout_amount(amount: Option<i64>, configured: Option<i64>) -> Result<i64, ConsoleError> {
    if let Some(amount) = amount {
        if amount < RELOAD_AMOUNT_MIN {
            return Err(ConsoleError::Validation(format!(
                "Amount must be at least ${RELOAD_AMOUNT_MIN}"
            )));
        }
        return Ok(amount);
    }
    Ok(configured.unwrap_or(RELOAD_AMOUNT))
}

/// Reproduces `Billing.calculateFeeInCents`, whose input/output are integer cents.
pub fn calculate_fee_in_cents(amount: i64) -> i64 {
    let amount = Decimal::from(amount);
    let fee = ((amount + Decimal::from(30)) / Decimal::new(956, 3) * Decimal::new(44, 3)
        + Decimal::from(30))
    .round();
    i64::try_from(fee).unwrap_or(i64::MAX)
}

/// Validates monthly limit input, matching the UI's nonnegative integer contract.
pub fn validate_monthly_limit(limit: i64) -> Result<(), ConsoleError> {
    if limit < 0 {
        return Err(ConsoleError::Validation(
            "Monthly limit must be greater than or equal to 0".into(),
        ));
    }
    Ok(())
}

/// Validates reload settings only when automatic reload is enabled, as in the baseline form.
pub fn validate_reload_settings(
    enabled: bool,
    amount: Option<i64>,
    trigger: Option<i64>,
) -> Result<(), ConsoleError> {
    if !enabled {
        return Ok(());
    }
    if amount.is_none_or(|value| value < RELOAD_AMOUNT_MIN) {
        return Err(ConsoleError::Validation(format!(
            "Amount must be at least ${RELOAD_AMOUNT_MIN}"
        )));
    }
    if trigger.is_none_or(|value| value < RELOAD_TRIGGER_MIN) {
        return Err(ConsoleError::Validation(format!(
            "Trigger must be at least ${RELOAD_TRIGGER_MIN}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_matches_baseline_examples() {
        assert_eq!(calculate_fee_in_cents(2_000), 123);
        assert_eq!(calculate_fee_in_cents(1_000), 77);
        assert_eq!(calculate_fee_in_cents(0), 31);
    }

    #[test]
    fn checkout_amount_applies_default_and_minimum() {
        assert_eq!(checkout_amount(None, None).unwrap(), 20);
        assert_eq!(checkout_amount(None, Some(35)).unwrap(), 35);
        assert_eq!(checkout_amount(Some(10), None).unwrap(), 10);
        assert_eq!(
            checkout_amount(Some(9), None).unwrap_err().to_string(),
            "Amount must be at least $10"
        );
    }

    #[test]
    fn limits_and_reload_rules_match_form_validation() {
        assert!(validate_monthly_limit(0).is_ok());
        assert!(validate_monthly_limit(-1).is_err());
        assert!(validate_reload_settings(false, None, None).is_ok());
        assert!(validate_reload_settings(true, Some(10), Some(5)).is_ok());
        assert!(validate_reload_settings(true, Some(9), Some(5)).is_err());
        assert!(validate_reload_settings(true, Some(10), None).is_err());
    }
}
