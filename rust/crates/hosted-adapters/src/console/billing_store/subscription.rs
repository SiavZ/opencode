//! Purpose: project active subscription counters and mutate baseline subscription preferences.
//! Scope: billing subscription queries, waitlist removal and balance opt-in persistence.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/billing_store.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing/subscription.rs
//! Invariants: deleted counters never appear; balance preferences preserve unrelated Black subscription fields.
//! Contract tests: hosted-adapters/tests/console_commerce_lifecycle.rs

use super::{BillingResult, BillingStore};
use mysql_async::prelude::Queryable;
use time::PrimitiveDateTime;

pub struct SubscriptionUsage {
    pub user_id: String,
    pub rolling_usage: Option<i64>,
    pub weekly_usage: Option<i64>,
    pub monthly_usage: Option<i64>,
    pub time_rolling_updated: Option<PrimitiveDateTime>,
    pub time_weekly_updated: Option<PrimitiveDateTime>,
    pub time_monthly_updated: Option<PrimitiveDateTime>,
    pub time_created: PrimitiveDateTime,
    pub settings: Option<serde_json::Value>,
    pub region: Option<serde_json::Value>,
    pub allow_training: bool,
}

impl BillingStore {
    pub async fn subscription_usage(
        &self,
        workspace: &str,
        lite: bool,
    ) -> BillingResult<Option<SubscriptionUsage>> {
        let mut conn = self.db.conn().await?;
        let (table, columns, settings) = if lite {
            (
                "lite",
                "s.weekly_usage,s.monthly_usage,s.time_weekly_updated,s.time_monthly_updated",
                "b.lite",
            )
        } else {
            (
                "subscription",
                "s.fixed_usage,NULL,s.time_fixed_updated,NULL",
                "b.subscription",
            )
        };
        let row: Option<mysql_async::Row> = conn.exec_first(format!("SELECT s.user_id,s.rolling_usage,s.time_rolling_updated,{columns},s.time_created,{settings},w.region,w.allow_training FROM `{table}` s JOIN billing b ON b.workspace_id=s.workspace_id JOIN workspace w ON w.id=s.workspace_id WHERE s.workspace_id=? AND s.time_deleted IS NULL LIMIT 1"),(workspace,)).await?;
        row.map(|mut row| {
            Ok(SubscriptionUsage {
                user_id: row.take(0).unwrap_or_default(),
                rolling_usage: row.take(1).flatten(),
                time_rolling_updated: row.take(2).flatten(),
                weekly_usage: row.take(3).flatten(),
                monthly_usage: row.take(4).flatten(),
                time_weekly_updated: row.take(5).flatten(),
                time_monthly_updated: row.take(6).flatten(),
                time_created: row.take(7).unwrap_or(PrimitiveDateTime::MIN),
                settings: row
                    .take::<Option<String>, _>(8)
                    .flatten()
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                region: row
                    .take::<Option<String>, _>(9)
                    .flatten()
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                allow_training: row.take::<Option<bool>, _>(10).flatten().unwrap_or(false),
            })
        })
        .transpose()
    }

    pub async fn cancel_waitlist(&self, workspace: &str) -> BillingResult<()> {
        let mut conn = self.db.conn().await?;
        conn.exec_drop("UPDATE billing SET subscription_plan=NULL,time_subscription_booked=NULL,time_subscription_selected=NULL WHERE workspace_id=?",(workspace,)).await?;
        Ok(())
    }

    pub async fn set_use_balance(
        &self,
        workspace: &str,
        lite: bool,
        enabled: bool,
    ) -> BillingResult<()> {
        let mut conn = self.db.conn().await?;
        if lite {
            conn.exec_drop(
                "UPDATE billing SET lite=? WHERE workspace_id=?",
                (
                    if enabled {
                        "{\"useBalance\":true}"
                    } else {
                        "{}"
                    },
                    workspace,
                ),
            )
            .await?;
        } else {
            conn.exec_drop(if enabled {"UPDATE billing SET subscription=JSON_SET(subscription,'$.useBalance',true) WHERE workspace_id=?"} else {"UPDATE billing SET subscription=JSON_REMOVE(subscription,'$.useBalance') WHERE workspace_id=?"},(workspace,)).await?;
        }
        Ok(())
    }

    pub async fn lite_usage_sources(
        &self,
        workspace: &str,
        user: &str,
        start: PrimitiveDateTime,
    ) -> BillingResult<Vec<(String, Option<String>, String, String)>> {
        let mut conn = self.db.conn().await?;
        Ok(conn.exec("SELECT u.model,JSON_UNQUOTE(JSON_EXTRACT(u.enrichment,'$.costMultiplier')),CAST(SUM(u.cost) AS CHAR),CAST(SUM(CASE WHEN JSON_EXTRACT(u.enrichment,'$.costMultiplier') IS NOT NULL THEN ROUND(u.cost*CAST(JSON_UNQUOTE(JSON_EXTRACT(u.enrichment,'$.costMultiplier')) AS DECIMAL(20,8))) ELSE 0 END) AS CHAR) FROM `usage` u JOIN `key` k ON k.id=u.key_id AND k.workspace_id=u.workspace_id WHERE u.workspace_id=? AND k.user_id=? AND u.time_created>=? AND JSON_UNQUOTE(JSON_EXTRACT(u.enrichment,'$.plan'))='lite' GROUP BY u.model,JSON_UNQUOTE(JSON_EXTRACT(u.enrichment,'$.costMultiplier'))",(workspace,user,start)).await?)
    }
}
