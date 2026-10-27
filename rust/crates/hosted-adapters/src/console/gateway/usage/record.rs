//! Purpose: persist inference usage and update the selected billing source atomically.
//! Scope: tenant usage idempotency, balance and quota counters, durable automatic-reload claim.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! |-- hosted-adapters/src/console/gateway/usage/batch.rs
//! |-- hosted-adapters/src/console/gateway/usage/quota.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! |-- rust_decimal::Decimal
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! Invariants: usage insert, debit and quota counters share a transaction; exact replay never debits twice.
//! Contract tests: hosted-adapters/tests/console_gateway_usage.rs

use mysql_async::{TxOpts, params, prelude::Queryable};
use opencode_hosted_domain::console::common::{IdKind, create_id};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use super::{BillingContext, BillingSource, UsageError, UsageRecord, UsageService, now, quota};
use crate::console::formats::UsageInfo;

impl UsageService {
    pub async fn record(
        &self,
        context: &BillingContext,
        session: &str,
        model_id: &str,
        model: &Value,
        provider_id: &str,
        usage: &UsageInfo,
    ) -> Result<UsageRecord, UsageError> {
        let id = create_id(IdKind::Usage, None).map_err(UsageError::internal)?;
        self.record_with_id(context, session, model_id, model, provider_id, usage, &id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_with_id(
        &self,
        context: &BillingContext,
        session: &str,
        model_id: &str,
        model: &Value,
        provider_id: &str,
        usage: &UsageInfo,
        usage_id: &str,
    ) -> Result<UsageRecord, UsageError> {
        let at = now();
        let raw_cents = quota::calculate_cost_in_cents(model, usage, at)?;
        let raw_cost = Decimal::from_f64_retain(raw_cents * 1_000_000.0)
            .ok_or_else(|| UsageError::internal("Usage cost overflow"))?;
        let cost = quota::round_money(raw_cost)
            .ok_or_else(|| UsageError::internal("Usage cost overflow"))?;
        let occurred = if context.source == BillingSource::Balance {
            format!(
                "{:.8}",
                Decimal::from_f64_retain(raw_cents / 100.0)
                    .ok_or_else(|| UsageError::internal("Usage cost overflow"))?
                    .round_dp_with_strategy(8, RoundingStrategy::MidpointAwayFromZero)
            )
        } else {
            "0".into()
        };
        let mut result = UsageRecord {
            occurred_cost: occurred,
            cost_in_microcents: cost,
            reload_claimed: false,
            raw_cost_in_cents: raw_cents,
            tracked_at: at,
        };
        if context.source == BillingSource::Anonymous {
            return Ok(result);
        }
        let workspace = context
            .workspace_id
            .as_deref()
            .ok_or_else(|| UsageError::internal("missing workspace"))?;
        let user = context
            .user_id
            .as_deref()
            .ok_or_else(|| UsageError::internal("missing user"))?;
        let key = context
            .key_id
            .as_deref()
            .ok_or_else(|| UsageError::internal("missing key"))?;
        if usage_id.is_empty() || usage_id.len() > 30 {
            return Err(UsageError::new(
                400,
                "ValidationError",
                "Invalid usage identifier",
            ));
        }
        let session: String = session.chars().take(30).collect();
        let enrichment = match context.source {
            BillingSource::Subscription => Some(json!({"plan":"sub"}).to_string()),
            BillingSource::Byok => Some(json!({"plan":"byok"}).to_string()),
            BillingSource::Lite => {
                Some(json!({"plan":"lite","costMultiplier":model.get("costMultiplier").cloned().unwrap_or(json!(1))}).to_string())
            }
            _ => None,
        };
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let billing: Option<(i64,)> = tx
            .exec_first(
                if workspace == super::batch::HOT_WORKSPACE {
                    "SELECT balance FROM billing WHERE workspace_id=?"
                } else {
                    "SELECT balance FROM billing WHERE workspace_id=? FOR UPDATE"
                },
                (workspace,),
            )
            .await?;
        if billing.is_none() {
            return Err(UsageError::new(401, "AuthError", "Invalid API key."));
        }
        // Ordinary workspaces serialize on billing. Hot requests avoid that row lock until an elected flush.
        let previous: Option<mysql_async::Row> = tx.exec_first("SELECT model,provider,key_id,session_id,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_5m_tokens,cache_write_1h_tokens,cost,enrichment,time_created FROM `usage` WHERE workspace_id=? AND id=?", (workspace, usage_id)).await?;
        if let Some(previous) = previous {
            let same = previous.get::<String, _>("model").as_deref() == Some(model_id)
                && previous.get::<String, _>("provider").as_deref() == Some(provider_id)
                && previous
                    .get::<Option<String>, _>("key_id")
                    .flatten()
                    .as_deref()
                    == Some(key)
                && previous
                    .get::<Option<String>, _>("session_id")
                    .flatten()
                    .as_deref()
                    == Some(session.as_str())
                && previous.get::<i64, _>("input_tokens") == Some(usage.input_tokens)
                && previous.get::<i64, _>("output_tokens") == Some(usage.output_tokens)
                && previous.get::<Option<i64>, _>("reasoning_tokens").flatten()
                    == usage.reasoning_tokens
                && previous
                    .get::<Option<i64>, _>("cache_read_tokens")
                    .flatten()
                    == usage.cache_read_tokens
                && previous
                    .get::<Option<i64>, _>("cache_write_5m_tokens")
                    .flatten()
                    == usage.cache_write5m_tokens
                && previous
                    .get::<Option<i64>, _>("cache_write_1h_tokens")
                    .flatten()
                    == usage.cache_write1h_tokens;
            if !same {
                return Err(UsageError::new(
                    409,
                    "ConflictError",
                    "Usage identifier reused with different request",
                ));
            }
            let previous_enrichment = previous
                .get::<Option<String>, _>("enrichment")
                .flatten()
                .map(|value| serde_json::from_str::<Value>(&value))
                .transpose()
                .map_err(UsageError::internal)?;
            if previous_enrichment
                != enrichment
                    .as_deref()
                    .map(serde_json::from_str::<Value>)
                    .transpose()
                    .map_err(UsageError::internal)?
            {
                return Err(UsageError::new(
                    409,
                    "ConflictError",
                    "Usage identifier reused with different billing source",
                ));
            }
            result.cost_in_microcents = previous
                .get("cost")
                .ok_or_else(|| UsageError::internal("missing stored usage cost"))?;
            result.tracked_at = previous
                .get("time_created")
                .ok_or_else(|| UsageError::internal("missing usage timestamp"))?;
            result.raw_cost_in_cents =
                quota::calculate_cost_in_cents(model, usage, result.tracked_at)?;
            result.occurred_cost = if context.source == BillingSource::Balance {
                format!(
                    "{:.8}",
                    Decimal::from_f64_retain(result.raw_cost_in_cents / 100.0)
                        .ok_or_else(|| UsageError::internal("Usage cost overflow"))?
                        .round_dp_with_strategy(8, RoundingStrategy::MidpointAwayFromZero)
                )
            } else {
                "0".into()
            };
            tx.rollback().await?;
            return Ok(result);
        }
        let owner: Option<String> = tx
            .exec_first(
                "SELECT id FROM `key` WHERE workspace_id=? AND id=? AND user_id=?",
                (workspace, key, user),
            )
            .await?;
        if owner.is_none() {
            return Err(UsageError::new(401, "AuthError", "Invalid API key."));
        }
        let inserted = tx.exec_drop("INSERT INTO `usage` (workspace_id,id,model,provider,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_5m_tokens,cache_write_1h_tokens,cost,key_id,session_id,enrichment,time_created) VALUES (:workspace,:id,:model,:provider,:input,:output,:reasoning,:read,:write5,:write1,:cost,:key,:session,:enrichment,:at)", params! {"workspace"=>workspace,"id"=>usage_id,"model"=>model_id,"provider"=>provider_id,"input"=>usage.input_tokens,"output"=>usage.output_tokens,"reasoning"=>usage.reasoning_tokens,"read"=>usage.cache_read_tokens,"write5"=>usage.cache_write5m_tokens,"write1"=>usage.cache_write1h_tokens,"cost"=>cost,"key"=>key,"session"=>&session,"enrichment"=>enrichment,"at"=>at}).await;
        if let Err(error) = inserted {
            let duplicate =
                matches!(&error,mysql_async::Error::Server(error) if error.code == 1062);
            tx.rollback().await?;
            if duplicate {
                // The winner committed before MySQL reports a duplicate. A new snapshot validates its payload.
                return Box::pin(self.record_with_id(
                    context,
                    &session,
                    model_id,
                    model,
                    provider_id,
                    usage,
                    usage_id,
                ))
                .await;
            }
            return Err(error.into());
        }
        if matches!(
            context.source,
            BillingSource::Subscription | BillingSource::Lite
        ) {
            let lite = context.source == BillingSource::Lite;
            let table = if lite { "lite" } else { "subscription" };
            let weekly = if lite { "weekly" } else { "fixed" };
            let limits = if lite {
                &self.limits["lite"]
            } else {
                &self.limits["black"][context.subscription["plan"].as_str().unwrap_or_default()]
            };
            let seconds = quota::limit(limits, "rollingWindow")?
                .checked_mul(3600)
                .ok_or_else(|| UsageError::internal("rolling window overflow"))?;
            let quota_cost = if lite {
                let multiplier = model
                    .get("costMultiplier")
                    .map_or(Some(1.0), Value::as_f64)
                    .ok_or_else(|| UsageError::internal("invalid model cost multiplier"))?;
                Decimal::from_f64_retain(cost as f64 * multiplier)
                    .and_then(quota::round_money)
                    .ok_or_else(|| UsageError::internal("quota cost overflow"))?
            } else {
                cost
            };
            let week = quota::week(at);
            tx.exec_drop(format!("UPDATE `{table}` SET {weekly}_usage=CASE WHEN time_{weekly}_updated>=:end THEN {weekly}_usage WHEN time_{weekly}_updated>=:start THEN {weekly}_usage + :cost ELSE :cost END,time_{weekly}_updated=CASE WHEN time_{weekly}_updated>:at THEN time_{weekly}_updated ELSE :at END,rolling_usage=CASE WHEN time_rolling_updated>=TIMESTAMPADD(SECOND,- :seconds,:at) THEN rolling_usage + :cost ELSE :cost END,time_rolling_updated=CASE WHEN time_rolling_updated>=TIMESTAMPADD(SECOND,- :seconds,:at) THEN time_rolling_updated ELSE :at END WHERE workspace_id=:workspace AND user_id=:user"), params! {"start"=>week.0,"end"=>week.1,"cost"=>quota_cost,"at"=>at,"seconds"=>seconds,"workspace"=>workspace,"user"=>user}).await?;
            if lite {
                let created = context
                    .lite_usage
                    .as_ref()
                    .ok_or_else(|| UsageError::internal("missing lite entitlement"))?
                    .created;
                let month = quota::month(at, created);
                tx.exec_drop("UPDATE lite SET monthly_usage=CASE WHEN time_monthly_updated>=:end THEN monthly_usage WHEN time_monthly_updated>=:start THEN monthly_usage + :cost ELSE :cost END,time_monthly_updated=CASE WHEN time_monthly_updated>:at THEN time_monthly_updated ELSE :at END WHERE workspace_id=:workspace AND user_id=:user", params! {"start"=>month.0,"end"=>month.1,"cost"=>quota_cost,"at"=>at,"workspace"=>workspace,"user"=>user}).await?;
            }
        } else {
            let non_debiting = matches!(context.source, BillingSource::Free | BillingSource::Byok);
            let deltas = if workspace == super::batch::HOT_WORKSPACE {
                let batcher = self.batcher.as_ref().ok_or_else(|| {
                    UsageError::new(
                        500,
                        "ConfigurationError",
                        "Hot workspace usage requires Redis",
                    )
                })?;
                batcher
                    .accumulate(workspace, user, if non_debiting { 0 } else { cost }, cost)
                    .await?
            } else {
                Some((cost, cost))
            };
            if let Some((workspace_delta, user_delta)) = deltas {
                let debit = if non_debiting { 0 } else { workspace_delta };
                tx.exec_drop("UPDATE billing SET balance=balance - :debit,monthly_usage=CASE WHEN YEAR(time_monthly_usage_updated)=YEAR(:at) AND MONTH(time_monthly_usage_updated)=MONTH(:at) THEN monthly_usage + :cost ELSE :cost END,time_monthly_usage_updated=:at WHERE workspace_id=:workspace", params! {"debit"=>debit,"cost"=>workspace_delta,"at"=>at,"workspace"=>workspace}).await?;
                tx.exec_drop("UPDATE user SET monthly_usage=CASE WHEN YEAR(time_monthly_usage_updated)=YEAR(:at) AND MONTH(time_monthly_usage_updated)=MONTH(:at) THEN monthly_usage + :cost ELSE :cost END,time_monthly_usage_updated=:at WHERE workspace_id=:workspace AND id=:user", params! {"cost"=>user_delta,"at"=>at,"workspace"=>workspace,"user"=>user}).await?;
            }
        }
        let reload_snapshot_allows = context.auth_row.as_ref().is_some_and(|row| {
            let balance = row.get::<i64, _>("balance").unwrap_or_default();
            let trigger = row
                .get::<Option<i64>, _>("reload_trigger")
                .flatten()
                .unwrap_or(5);
            let unlocked = row
                .get::<Option<time::PrimitiveDateTime>, _>("time_reload_locked_till")
                .flatten()
                .is_none_or(|locked| locked <= at);
            // Legacy precheck subtracts raw cents from the microcent snapshot; the DB claim compares microcents.
            unlocked
                && Decimal::from(balance)
                    - Decimal::from_f64_retain(raw_cents).unwrap_or(Decimal::ZERO)
                    < Decimal::from(trigger) * Decimal::from(100_000_000)
        });
        if context.source == BillingSource::Balance
            && self.stripe.is_some()
            && reload_snapshot_allows
        {
            tx.exec_drop("UPDATE billing SET time_reload_locked_till=TIMESTAMPADD(MINUTE,1,:at) WHERE workspace_id=:workspace AND reload=TRUE AND balance<COALESCE(reload_trigger,5)*100000000 AND (time_reload_locked_till IS NULL OR time_reload_locked_till<:at)", params! {"at"=>at,"workspace"=>workspace}).await?;
            result.reload_claimed = tx.affected_rows() != 0;
        }
        tx.commit().await?;
        if result.reload_claimed {
            self.perform_reload(workspace).await?;
        }
        Ok(result)
    }

    async fn perform_reload(&self, workspace: &str) -> Result<(), UsageError> {
        let Some(stripe) = &self.stripe else {
            return Ok(());
        };
        let mut conn = self.db.conn().await?;
        let row: Option<(Option<String>, Option<String>, Option<i64>)> = conn.exec_first("SELECT customer_id,payment_method_id,reload_amount FROM billing WHERE workspace_id=?", (workspace,)).await?;
        let (customer, payment, amount) =
            row.ok_or_else(|| UsageError::internal("missing reload billing"))?;
        let result = stripe
            .reload(
                customer.as_deref().unwrap_or_default(),
                payment.as_deref().unwrap_or_default(),
                workspace,
                amount.unwrap_or(20),
            )
            .await;
        if let Err(error) = result {
            conn.exec_drop("UPDATE billing SET reload=FALSE,reload_error=?,time_reload_error=NOW(3) WHERE workspace_id=?", (error.to_string(), workspace)).await?;
        }
        Ok(())
    }
}
