//! Purpose: store inference trial, affinity and provider performance counters in baseline MySQL.
//! Scope: request-time routing snapshots and completed-response updates.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::Value
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: trial usage counts all token classes; TPM counts only uncached input; TPS needs over ten tokens.
//! Contract tests: hosted-adapters/tests/console_gateway_limits.rs

use super::interval;
use crate::console::{
    db::{ConsoleDb, DbError},
    formats::UsageInfo,
};
use mysql_async::prelude::Queryable;
use serde_json::Value;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct ProviderCounters {
    db: ConsoleDb,
    pub minute: i64,
    previous: i64,
}

#[derive(Default)]
pub struct RoutingSnapshot {
    pub trial: bool,
    pub sticky: Option<String>,
    pub tpm: HashMap<String, i64>,
    pub tps: HashMap<String, (i64, i64)>,
}

impl ProviderCounters {
    pub fn new(db: ConsoleDb, now: OffsetDateTime) -> Self {
        Self {
            db,
            minute: interval(now).parse().expect("numeric interval"),
            previous: interval(now - time::Duration::minutes(1))
                .parse()
                .expect("numeric interval"),
        }
    }

    pub async fn snapshot(
        &self,
        model: &Value,
        model_id: &str,
        sticky_id: &str,
        ip: &str,
        promo_tokens: i64,
    ) -> Result<RoutingSnapshot, DbError> {
        let mut conn = self.db.conn().await?;
        let mut result = RoutingSnapshot::default();
        if model.get("trialProvider").is_some() && !ip.is_empty() {
            let usage: Option<Option<i64>> = conn
                .exec_first("SELECT `usage` FROM ip WHERE ip = ?", (ip,))
                .await?;
            result.trial = usage.flatten().unwrap_or_default() < promo_tokens;
        }
        if model.get("stickyProvider").is_some() && !sticky_id.is_empty() {
            result.sticky = conn
                .exec_first(
                    "SELECT provider_id FROM model_sticky_provider WHERE id = ? LIMIT 1",
                    (format!("{model_id}/{sticky_id}"),),
                )
                .await?;
        }
        for provider in model
            .get("providers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = provider
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let native = provider
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if provider
                .get("tpmLimit")
                .and_then(Value::as_f64)
                .is_some_and(|v| v != 0.0)
            {
                let key = format!("{id}/{native}");
                let count: Option<i64> = conn
                    .exec_first(
                        "SELECT count FROM model_tpm_rate_limit WHERE id = ? AND `interval` = ?",
                        (&key, self.minute),
                    )
                    .await?;
                result.tpm.insert(key, count.unwrap_or_default());
            }
            if let Some(goal) = provider
                .get("tpsGoal")
                .filter(|v| v.as_f64().is_some_and(|v| v != 0.0))
            {
                let key = format!("{id}/{native}/{goal}");
                let counts: Vec<(i64, i64)> = conn.exec("SELECT `qualify`, `unqualify` FROM model_tps_rate_limit WHERE id = ? AND `interval` IN (?, ?)", (&key, self.minute, self.previous)).await?;
                result.tps.insert(
                    key,
                    counts
                        .iter()
                        .fold((0, 0), |sum, row| (sum.0 + row.0, sum.1 + row.1)),
                );
            }
        }
        Ok(result)
    }

    pub async fn sticky(
        &self,
        model: &str,
        sticky_id: &str,
        provider: &str,
    ) -> Result<(), DbError> {
        let mut conn = self.db.conn().await?;
        conn.exec_drop("INSERT INTO model_sticky_provider (id, provider_id) VALUES (?, ?) ON DUPLICATE KEY UPDATE provider_id = VALUES(provider_id)", (format!("{model}/{sticky_id}"), provider)).await?;
        Ok(())
    }

    pub async fn track(
        &self,
        provider: &Value,
        ip: &str,
        trial: bool,
        usage: &UsageInfo,
        first_byte_ms: i64,
        last_byte_ms: i64,
    ) -> Result<(), DbError> {
        let mut conn = self.db.conn().await?;
        if trial {
            let total = usage.input_tokens
                + usage.output_tokens
                + usage.reasoning_tokens.unwrap_or_default()
                + usage.cache_read_tokens.unwrap_or_default()
                + usage.cache_write5m_tokens.unwrap_or_default()
                + usage.cache_write1h_tokens.unwrap_or_default();
            conn.exec_drop("INSERT INTO ip (ip, `usage`) VALUES (?, ?) ON DUPLICATE KEY UPDATE `usage` = `usage` + VALUES(`usage`)", (ip, total)).await?;
        }
        let id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let native = provider
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if provider
            .get("tpmLimit")
            .and_then(Value::as_f64)
            .is_some_and(|v| v != 0.0)
            && usage.input_tokens > 0
        {
            conn.exec_drop("INSERT INTO model_tpm_rate_limit (id, `interval`, count) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE count = count + VALUES(count)", (format!("{id}/{native}"), self.minute, usage.input_tokens)).await?;
        }
        if let Some(goal) = provider
            .get("tpsGoal")
            .and_then(Value::as_f64)
            .filter(|goal| *goal != 0.0)
            && first_byte_ms > 0
            && last_byte_ms > 0
            && usage.output_tokens > 10
        {
            let tps = usage.output_tokens as f64 / (last_byte_ms - first_byte_ms) as f64 * 1000.0;
            let goal_value = &provider["tpsGoal"];
            conn.exec_drop("INSERT INTO model_tps_rate_limit (id, `interval`, `qualify`, `unqualify`) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE `qualify` = `qualify` + VALUES(`qualify`), `unqualify` = `unqualify` + VALUES(`unqualify`)", (format!("{id}/{native}/{goal_value}"), self.minute, i64::from(tps >= goal), i64::from(tps < goal))).await?;
        }
        Ok(())
    }
}
