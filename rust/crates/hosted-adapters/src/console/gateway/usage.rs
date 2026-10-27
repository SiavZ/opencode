//! Purpose: authenticate and account for legacy Zen inference using durable Console billing rows.
//! Scope: billing source validation, usage recording, reload and Go usage readback.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/gateway/usage/auth.rs
//! |-- hosted-adapters/src/console/gateway/usage/batch.rs
//! |-- hosted-adapters/src/console/gateway/usage/quota.rs
//! |-- hosted-adapters/src/console/gateway/usage/record.rs
//! |-- hosted-adapters/src/console/limits.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- serde_json::Value
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway.rs
//! Invariants: integer microcents are persisted once per tenant and usage identifier; anonymous usage has no billing writes.
//! Contract tests: hosted-adapters/tests/console_gateway_usage.rs

mod auth;
mod batch;
mod quota;
mod record;

pub use batch::UsageBatcher;
pub use quota::{
    calculate_cost, calculate_cost_components, calculate_cost_decimal, calculate_cost_in_cents,
};

use serde_json::{Value, json};
use time::PrimitiveDateTime;

use crate::console::db::{ConsoleDb, DbError};
use crate::console::stripe::StripeClient;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingSource {
    Anonymous,
    Byok,
    Free,
    Subscription,
    Lite,
    Balance,
}

impl BillingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::Byok => "byok",
            Self::Free => "free",
            Self::Subscription => "subscription",
            Self::Lite => "lite",
            Self::Balance => "balance",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct UsageError {
    pub status: u16,
    pub kind: String,
    pub message: String,
    pub retry_after: Option<i64>,
    pub metadata: Box<Value>,
    diagnostic: Option<String>,
}

impl UsageError {
    pub fn response_json(&self) -> Value {
        let mut value = json!({"type":"error","error":{"type":self.kind,"message":self.message}});
        if self.status == 429 {
            value["metadata"] = self.metadata.as_ref().clone();
        }
        value
    }

    fn new(status: u16, kind: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind: kind.into(),
            message: message.into(),
            retry_after: None,
            metadata: Box::new(json!({})),
            diagnostic: None,
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!("Console usage operation failed");
        let mut result = Self::new(500, "error", "Internal server error");
        result.diagnostic = Some(error.to_string());
        result
    }
}

impl From<DbError> for UsageError {
    fn from(value: DbError) -> Self {
        Self::internal(value)
    }
}

impl From<mysql_async::Error> for UsageError {
    fn from(value: mysql_async::Error) -> Self {
        Self::internal(value)
    }
}

#[derive(Clone, Debug)]
pub struct BillingContext {
    pub workspace_id: Option<String>,
    pub provider_credentials: Option<String>,
    pub source: BillingSource,
    pub allow_training: bool,
    pub region: Value,
    key_id: Option<String>,
    user_id: Option<String>,
    subscription: Value,
    lite: Value,
    black_usage: Option<Counters>,
    lite_usage: Option<Counters>,
    auth_row: Option<mysql_async::Row>,
}

impl BillingContext {
    pub fn subscription_plan(&self) -> Option<&str> {
        if self.subscription.is_object() {
            return self.subscription["plan"].as_str();
        }
        self.lite.is_object().then_some("lite")
    }
    pub fn key_id(&self) -> Option<&str> {
        self.key_id.as_deref()
    }
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }
}

#[derive(Clone, Debug)]
struct Counters {
    created: PrimitiveDateTime,
    rolling: Option<i64>,
    weekly: Option<i64>,
    monthly: Option<i64>,
    rolling_at: Option<PrimitiveDateTime>,
    weekly_at: Option<PrimitiveDateTime>,
    monthly_at: Option<PrimitiveDateTime>,
}

#[derive(Clone, Debug)]
pub struct UsageService {
    db: ConsoleDb,
    limits: Value,
    stripe: Option<StripeClient>,
    production: bool,
    batcher: Option<UsageBatcher>,
}

impl UsageService {
    pub fn new(db: ConsoleDb, limits: Value) -> Self {
        Self {
            db,
            limits,
            stripe: None,
            production: false,
            batcher: None,
        }
    }

    pub fn with_stripe(mut self, stripe: StripeClient) -> Self {
        self.stripe = Some(stripe);
        self
    }

    pub fn with_production(mut self, production: bool) -> Self {
        self.production = production;
        self
    }

    pub fn with_usage_batcher(mut self, redis: crate::console::limits::RedisCounters) -> Self {
        self.batcher = Some(UsageBatcher::new(redis));
        self
    }

    pub fn with_flush_probability(mut self, probability: f64) -> Result<Self, UsageError> {
        if !(0.0..=1.0).contains(&probability) {
            return Err(UsageError::new(
                400,
                "ValidationError",
                "Invalid usage flush probability",
            ));
        }
        let batcher = self.batcher.as_mut().ok_or_else(|| {
            UsageError::new(500, "ConfigurationError", "Usage batcher not configured")
        })?;
        batcher.flush_probability = probability;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub occurred_cost: String,
    pub cost_in_microcents: i64,
    pub reload_claimed: bool,
    pub raw_cost_in_cents: f64,
    pub tracked_at: PrimitiveDateTime,
}

fn now() -> PrimitiveDateTime {
    let now = time::OffsetDateTime::now_utc();
    PrimitiveDateTime::new(now.date(), now.time())
        .replace_nanosecond(u32::from(now.millisecond()) * 1_000_000)
        .expect("valid millisecond timestamp")
}
