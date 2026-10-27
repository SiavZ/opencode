//! Purpose: consume referral rewards and preview their effect on active Lite quota counters.
//! Scope: transactional reward ledger and usage-counter updates.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/referral.rs
//! `-- hosted-domain/src/console/quotas.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/referral.rs
//! Invariants: reward consumption and all counter reductions commit together; failed subscription lookup never consumes reward.
//! Contract tests: hosted-adapters/tests/console_commerce_referral.rs

use super::*;
use opencode_hosted_domain::console::{common::microcents_to_cents, quotas};
use serde_json::{Value, json};
use time::PrimitiveDateTime;

impl ReferralStore {
    pub async fn apply_reward(&self, workspace: &str, referral: &str) -> ReferralResult<Value> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let reward:Option<(i64,Option<PrimitiveDateTime>)>=tx.exec_first("SELECT amount,time_applied FROM referral_reward WHERE workspace_id=? AND referral_id=? AND time_deleted IS NULL FOR UPDATE",(workspace,referral)).await?;
        let Some((amount, applied)) = reward else {
            return Err(ConsoleError::NotFound("Referral reward not found".into()).into());
        };
        if applied.is_some() {
            return Err(ConsoleError::Conflict("Referral reward already applied".into()).into());
        }
        let lite:Option<String>=tx.exec_first("SELECT id FROM lite WHERE workspace_id=? AND time_deleted IS NULL LIMIT 1 FOR UPDATE",(workspace,)).await?;
        if lite.is_none() {
            return Err(ConsoleError::Validation(
                "Subscribe to Go before applying referral rewards".into(),
            )
            .into());
        }
        tx.exec_drop("UPDATE lite SET monthly_usage=GREATEST(0,COALESCE(monthly_usage,0)-?),weekly_usage=GREATEST(0,COALESCE(weekly_usage,0)-?),rolling_usage=GREATEST(0,COALESCE(rolling_usage,0)-?) WHERE workspace_id=? AND time_deleted IS NULL",(amount,amount,amount,workspace)).await?;
        tx.exec_drop("UPDATE referral_reward SET time_applied=NOW(3) WHERE workspace_id=? AND referral_id=? AND time_applied IS NULL AND time_deleted IS NULL",(workspace,referral)).await?;
        tx.commit().await?;
        Ok(json!({"amount":microcents_to_cents(amount)}))
    }

    pub async fn usage_preview(
        &self,
        workspace: &str,
        referral: &str,
        limits: &Value,
    ) -> ReferralResult<Value> {
        let mut conn = self.db.conn().await?;
        let row:Option<mysql_async::Row>=conn.exec_first("SELECT r.amount,l.rolling_usage,l.weekly_usage,l.monthly_usage,l.time_rolling_updated,l.time_weekly_updated,l.time_monthly_updated,l.time_created FROM referral_reward r JOIN lite l ON l.workspace_id=r.workspace_id WHERE r.workspace_id=? AND r.referral_id=? AND r.time_applied IS NULL AND r.time_deleted IS NULL AND l.time_deleted IS NULL LIMIT 1",(workspace,referral)).await?;
        let Some(mut row) = row else {
            return Ok(Value::Null);
        };
        let amount: i64 = row.take(0).unwrap_or_default();
        let rolling: Option<i64> = row.take(1).flatten();
        let weekly: Option<i64> = row.take(2).flatten();
        let monthly: Option<i64> = row.take(3).flatten();
        let rolling_at: Option<PrimitiveDateTime> = row.take(4).flatten();
        let weekly_at: Option<PrimitiveDateTime> = row.take(5).flatten();
        let monthly_at: Option<PrimitiveDateTime> = row.take(6).flatten();
        let subscribed: PrimitiveDateTime = row.take(7).unwrap_or(PrimitiveDateTime::MIN);
        let now = time::OffsetDateTime::now_utc();
        let now = PrimitiveDateTime::new(now.date(), now.time());
        let limits = limits.get("lite").unwrap_or(limits);
        let limit = |name: &str| {
            limits[name]
                .as_i64()
                .filter(|value| *value >= 0)
                .ok_or_else(|| {
                    ConsoleError::Validation(format!(
                        "Missing or invalid subscription limit {name}"
                    ))
                })
        };
        let after =
            |usage: Option<i64>| Some(usage.unwrap_or_default().saturating_sub(amount).max(0));
        let pair = |before: quotas::Quota, after: quotas::Quota| json!({"beforePercent":before.percent,"afterPercent":after.percent,"resetInSec":after.reset});
        let week = quotas::week(now);
        let month = quotas::month(now, subscribed);
        Ok(json!({
            "rollingUsage":pair(quotas::rolling(limit("rollingLimit")?,limit("rollingWindow")?,rolling,rolling_at,now),quotas::rolling(limit("rollingLimit")?,limit("rollingWindow")?,after(rolling),rolling_at,now)),
            "weeklyUsage":pair(quotas::analyze(limit("weeklyLimit")?,weekly,weekly_at.or(Some(now)),week.0,week.1,now),quotas::analyze(limit("weeklyLimit")?,after(weekly),weekly_at.or(Some(now)),week.0,week.1,now)),
            "monthlyUsage":pair(quotas::analyze(limit("monthlyLimit")?,monthly,monthly_at.or(Some(now)),month.0,month.1,now),quotas::analyze(limit("monthlyLimit")?,after(monthly),monthly_at.or(Some(now)),month.0,month.1,now))
        }))
    }
}
