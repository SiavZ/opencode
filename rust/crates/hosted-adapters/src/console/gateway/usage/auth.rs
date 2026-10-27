//! Purpose: resolve API-key billing identity and validate legacy Zen entitlements.
//! Scope: tenant joins, BYOK and free precedence, subscription and balance checks, Go usage.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-adapters/src/console/gateway/usage/quota.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::Value
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! Invariants: key owners join by workspace and user; only nondeleted entitlement rows qualify.
//! Contract tests: hosted-adapters/tests/console_gateway_usage.rs

use mysql_async::prelude::Queryable;
use serde_json::{Value, json};
use time::PrimitiveDateTime;

use super::{BillingContext, BillingSource, Counters, UsageError, UsageService, now, quota};

const ADMIN_WORKSPACES: [&str; 3] = [
    "wrk_01K46JDFR0E75SG2Q8K172KF3Y",
    "wrk_01K6W1A3VE0KMNVSCQT43BG2SX",
    "wrk_01KKZDKDWCS1VTJF8QTX62DD50",
];

impl UsageService {
    pub async fn default_region(
        &self,
        context: &mut BillingContext,
        country: Option<&str>,
    ) -> Result<(), UsageError> {
        if !context.region.is_null() {
            return Ok(());
        }
        let Some(workspace) = context.workspace_id.as_deref() else {
            return Ok(());
        };
        let region = if country.is_some_and(|country| country.eq_ignore_ascii_case("CN")) {
            json!(["us", "eu", "sg", "cn"])
        } else {
            json!(["us", "eu", "sg"])
        };
        let mut conn = self.db.conn().await?;
        conn.exec_drop(
            "UPDATE workspace SET region=? WHERE id=? AND region IS NULL",
            (region.to_string(), workspace),
        )
        .await?;
        // Match setDefaultRegion: this request uses its country choice even if another request won the write.
        context.region = region;
        Ok(())
    }

    pub async fn authenticate(
        &self,
        key: Option<&str>,
        model_id: &str,
        model: &Value,
        lite: bool,
    ) -> Result<BillingContext, UsageError> {
        let mut context = self
            .authenticate_identity(key, model_id, model, lite)
            .await?;
        self.validate_context(&mut context, model, lite)?;
        self.validate_model_settings(&context, model)?;
        Ok(context)
    }

    pub async fn authenticate_identity(
        &self,
        key: Option<&str>,
        model_id: &str,
        model: &Value,
        _lite: bool,
    ) -> Result<BillingContext, UsageError> {
        let anonymous = model["allowAnonymous"].as_bool().unwrap_or(false);
        let Some(key) = key.filter(|key| !key.is_empty() && *key != "public") else {
            if !anonymous {
                return Err(UsageError::new(401, "AuthError", "Missing API key."));
            }
            return Ok(BillingContext {
                workspace_id: None,
                provider_credentials: None,
                source: BillingSource::Anonymous,
                allow_training: false,
                region: Value::Null,
                key_id: None,
                user_id: None,
                subscription: Value::Null,
                lite: Value::Null,
                black_usage: None,
                lite_usage: None,
                auth_row: None,
            });
        };
        let mut conn = self.db.conn().await?;
        // The inference baseline intentionally checks key deletion only. Go usage below is stricter.
        let row: Option<mysql_async::Row> = conn.exec_first(
            "SELECT k.id AS key_id,k.workspace_id,k.user_id,w.region,w.allow_training,w.is_blocked,w.is_flagged_by_anthropic,w.is_flagged_by_openai,b.balance,b.payment_method_id,b.reload_trigger,b.time_reload_locked_till,b.monthly_limit AS billing_limit,b.monthly_usage AS billing_usage,b.time_monthly_usage_updated AS billing_updated,b.subscription,b.lite,u.monthly_limit AS user_limit,u.monthly_usage AS user_usage,u.time_monthly_usage_updated AS user_updated,p.credentials,m.time_created AS disabled FROM `key` k INNER JOIN workspace w ON w.id=k.workspace_id INNER JOIN billing b ON b.workspace_id=k.workspace_id INNER JOIN user u ON u.workspace_id=k.workspace_id AND u.id=k.user_id LEFT JOIN provider p ON p.workspace_id=k.workspace_id AND p.provider=? LEFT JOIN model m ON m.workspace_id=k.workspace_id AND m.model=? WHERE k.`key`=? AND k.time_deleted IS NULL LIMIT 1",
            (model["byokProvider"].as_str(), model_id, key),
        ).await?;
        let row = row.ok_or_else(|| UsageError::new(401, "AuthError", "Invalid API key."))?;
        let workspace: String = row
            .get("workspace_id")
            .ok_or_else(|| UsageError::internal("missing workspace identity"))?;
        let user: String = row
            .get("user_id")
            .ok_or_else(|| UsageError::internal("missing user identity"))?;
        let blocked = row
            .get::<Option<bool>, _>("is_blocked")
            .flatten()
            .unwrap_or(false)
            || (model_id.starts_with("claude-")
                && row
                    .get::<Option<bool>, _>("is_flagged_by_anthropic")
                    .flatten()
                    .unwrap_or(false))
            || (model_id.starts_with("gpt-")
                && row
                    .get::<Option<bool>, _>("is_flagged_by_openai")
                    .flatten()
                    .unwrap_or(false));
        if blocked {
            return Err(UsageError::new(
                401,
                "AuthError",
                "This account has found to be committing fraud or is in breach of terms of services and has been blocked. If you believe this is a mistake please send an email to help@anoma.ly.",
            ));
        }
        let free = ADMIN_WORKSPACES.contains(&workspace.as_str());
        if self.production && model_id.starts_with("alpha-") && !free {
            return Err(UsageError::new(
                401,
                "AuthError",
                format!("Model {model_id} is not supported"),
            ));
        }
        let subscription = json_column(&row, "subscription")?;
        let lite_subscription = json_column(&row, "lite")?;
        let black_usage = counters(&mut conn, "subscription", &workspace, &user).await?;
        let lite_usage = counters(&mut conn, "lite", &workspace, &user).await?;
        let context = BillingContext {
            workspace_id: Some(workspace.clone()),
            key_id: row.get("key_id"),
            user_id: Some(user),
            provider_credentials: row
                .get::<Option<String>, _>("credentials")
                .flatten()
                .filter(|value| !value.is_empty()),
            source: BillingSource::Balance,
            allow_training: row
                .get::<Option<bool>, _>("allow_training")
                .flatten()
                .unwrap_or(false),
            region: json_column(&row, "region")?,
            subscription,
            lite: lite_subscription,
            black_usage,
            lite_usage,
            auth_row: Some(row),
        };
        Ok(context)
    }

    pub fn validate_context(
        &self,
        context: &mut BillingContext,
        model: &Value,
        lite: bool,
    ) -> Result<(), UsageError> {
        let Some(row) = context.auth_row.as_ref() else {
            return Ok(());
        };
        let free = context
            .workspace_id
            .as_deref()
            .is_some_and(|workspace| ADMIN_WORKSPACES.contains(&workspace));
        let anonymous = model["allowAnonymous"].as_bool().unwrap_or(false);
        context.source = if context.provider_credentials.is_some() {
            BillingSource::Byok
        } else if free || anonymous {
            BillingSource::Free
        } else {
            self.validate_billing(context, row, model, lite, now())?
        };
        Ok(())
    }

    pub fn validate_model_settings(
        &self,
        context: &BillingContext,
        _model: &Value,
    ) -> Result<(), UsageError> {
        let Some(row) = context.auth_row.as_ref() else {
            return Ok(());
        };
        if !matches!(
            context.source,
            BillingSource::Lite | BillingSource::Anonymous
        ) && row
            .get::<Option<PrimitiveDateTime>, _>("disabled")
            .flatten()
            .is_some()
        {
            return Err(UsageError::new(401, "ModelError", "Model is disabled"));
        }
        Ok(())
    }

    fn validate_billing(
        &self,
        context: &BillingContext,
        row: &mysql_async::Row,
        model: &Value,
        lite: bool,
        at: PrimitiveDateTime,
    ) -> Result<BillingSource, UsageError> {
        if context.subscription.is_object() && context.black_usage.is_some() {
            let result = self.validate_quota(context, false, at);
            if result.is_ok() {
                return Ok(BillingSource::Subscription);
            }
            if let Err(error) = result
                && (!context.subscription["useBalance"]
                    .as_bool()
                    .unwrap_or(false)
                    || error.status != 429)
            {
                return Err(error);
            }
        }
        if lite && context.lite.is_object() && context.lite_usage.is_some() {
            if model["cost"]
                .as_object()
                .is_some_and(|prices| prices.values().all(|price| price.as_f64() == Some(0.0)))
            {
                return Ok(BillingSource::Lite);
            }
            let result = self.validate_quota(context, true, at);
            if result.is_ok() {
                return Ok(BillingSource::Lite);
            }
            if let Err(error) = result
                && (!context.lite["useBalance"].as_bool().unwrap_or(false) || error.status != 429)
            {
                return Err(error);
            }
        }
        let workspace = context.workspace_id.as_deref().unwrap_or_default();
        let billing_url = format!("https://opencode.ai/workspace/{workspace}/billing");
        let balance = row.get::<i64, _>("balance").unwrap_or_default();
        if balance <= 0 {
            let message = if row
                .get::<Option<String>, _>("payment_method_id")
                .flatten()
                .is_none_or(|value| value.is_empty())
            {
                format!("No payment method. Add a payment method here: {billing_url}")
            } else {
                format!("Insufficient balance. Manage your billing here: {billing_url}")
            };
            return Err(UsageError::new(401, "CreditsError", message));
        }
        for (prefix, kind) in [("billing", "MonthlyLimitError"), ("user", "UserLimitError")] {
            let limit = row
                .get::<Option<i64>, _>(format!("{prefix}_limit").as_str())
                .flatten();
            let usage = row
                .get::<Option<i64>, _>(format!("{prefix}_usage").as_str())
                .flatten();
            let updated = row
                .get::<Option<PrimitiveDateTime>, _>(format!("{prefix}_updated").as_str())
                .flatten();
            if let (Some(limit), Some(usage), Some(updated)) = (
                limit.filter(|value| *value != 0),
                usage.filter(|value| *value != 0),
                updated,
            ) && updated.year() == at.year()
                && updated.month() == at.month()
                && i128::from(usage) >= i128::from(limit) * 100_000_000
            {
                let message = if prefix == "billing" {
                    format!(
                        "Your workspace has reached its monthly spending limit of ${limit}. Manage your limits here: {billing_url}"
                    )
                } else {
                    format!(
                        "You have reached your monthly spending limit of ${limit}. Manage your limits here: https://opencode.ai/workspace/{workspace}/members"
                    )
                };
                return Err(UsageError::new(401, kind, message));
            }
        }
        Ok(BillingSource::Balance)
    }

    fn validate_quota(
        &self,
        context: &BillingContext,
        lite: bool,
        at: PrimitiveDateTime,
    ) -> Result<(), UsageError> {
        let counters = if lite {
            context.lite_usage.as_ref()
        } else {
            context.black_usage.as_ref()
        }
        .ok_or_else(|| UsageError::internal("missing entitlement"))?;
        let limits = if lite {
            &self.limits["lite"]
        } else {
            &self.limits["black"][context.subscription["plan"].as_str().unwrap_or_default()]
        };
        let week = quota::week(at);
        let weekly = quota::analyze(
            quota::limit(limits, if lite { "weeklyLimit" } else { "fixedLimit" })?,
            counters.weekly,
            counters.weekly_at,
            week.0,
            week.1,
            at,
        );
        let month = quota::month(at, counters.created);
        let monthly = if lite {
            Some(quota::analyze(
                quota::limit(limits, "monthlyLimit")?,
                counters.monthly,
                counters.monthly_at,
                month.0,
                month.1,
                at,
            ))
        } else {
            None
        };
        let rolling = quota::rolling(
            quota::limit(limits, "rollingLimit")?,
            quota::limit(limits, "rollingWindow")?,
            counters.rolling,
            counters.rolling_at,
            at,
        );
        for (name, analysis, usage, updated) in [
            ("weekly", Some(weekly), counters.weekly, counters.weekly_at),
            ("monthly", monthly, counters.monthly, counters.monthly_at),
            (
                "5 hour",
                Some(rolling),
                counters.rolling,
                counters.rolling_at,
            ),
        ] {
            // Legacy guards skip null and zero counters, even when the configured limit is zero.
            if !usage.is_some_and(|usage| usage != 0) || updated.is_none() {
                continue;
            }
            let Some(analysis) = analysis.filter(|analysis| analysis.limited) else {
                continue;
            };
            let retry = quota::retry_time(analysis.reset);
            let mut error = UsageError::new(
                429,
                if lite {
                    "GoUsageLimitError"
                } else {
                    "BlackUsageLimitError"
                },
                if lite {
                    format!(
                        "{} usage limit reached. Resets in {retry}. To continue using this model now, enable usage from your available balance: https://opencode.ai/workspace/{}/go",
                        match name {
                            "weekly" => "Weekly",
                            "monthly" => "Monthly",
                            _ => "5-hour",
                        },
                        context.workspace_id.as_deref().unwrap_or_default()
                    )
                } else {
                    format!("Subscription quota exceeded. Retry in {retry}.")
                },
            );
            error.retry_after = Some(analysis.reset);
            if lite {
                error.metadata =
                    Box::new(json!({"workspace":context.workspace_id,"limitName":name}));
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn go_usage(&self, key: Option<&str>) -> Result<Value, UsageError> {
        let key = key
            .filter(|key| !key.is_empty())
            .ok_or_else(|| UsageError::new(401, "AuthError", "Missing API key."))?;
        let mut conn = self.db.conn().await?;
        let auth: Option<(String, String)> = conn.exec_first("SELECT k.workspace_id,k.user_id FROM `key` k INNER JOIN user u ON u.workspace_id=k.workspace_id AND u.id=k.user_id AND u.time_deleted IS NULL INNER JOIN workspace w ON w.id=k.workspace_id AND w.time_deleted IS NULL INNER JOIN billing b ON b.workspace_id=k.workspace_id AND b.time_deleted IS NULL WHERE k.`key`=? AND k.time_deleted IS NULL LIMIT 1", (key,)).await?;
        let (workspace, user) =
            auth.ok_or_else(|| UsageError::new(401, "AuthError", "Unauthorized"))?;
        let counters = counters(&mut conn, "lite", &workspace, &user)
            .await?
            .ok_or_else(|| {
                UsageError::new(
                    403,
                    "EntitlementError",
                    "OpenCode Go subscription required.",
                )
            })?;
        let at = now();
        let limits = &self.limits["lite"];
        let week = quota::week(at);
        let month = quota::month(at, counters.created);
        Ok(json!({"usage":{
            "rolling":quota::rolling(quota::limit(limits,"rollingLimit")?,quota::limit(limits,"rollingWindow")?,counters.rolling,counters.rolling_at,at).json(at),
            "weekly":quota::analyze(quota::limit(limits,"weeklyLimit")?,counters.weekly,counters.weekly_at.or(Some(at)),week.0,week.1,at).json(at),
            "monthly":quota::analyze(quota::limit(limits,"monthlyLimit")?,counters.monthly,counters.monthly_at.or(Some(at)),month.0,month.1,at).json(at)
        }}))
    }
}

fn json_column(row: &mysql_async::Row, name: &str) -> Result<Value, UsageError> {
    row.get::<Option<String>, _>(name)
        .flatten()
        .map(|value| serde_json::from_str(&value).map_err(UsageError::internal))
        .transpose()
        .map(|value| value.unwrap_or(Value::Null))
}

async fn counters(
    conn: &mut mysql_async::Conn,
    table: &str,
    workspace: &str,
    user: &str,
) -> Result<Option<Counters>, UsageError> {
    let columns = if table == "lite" {
        "weekly_usage,monthly_usage,time_weekly_updated,time_monthly_updated"
    } else {
        "fixed_usage,NULL AS monthly_usage,time_fixed_updated,NULL AS time_monthly_updated"
    };
    let row: Option<mysql_async::Row> = conn.exec_first(format!("SELECT time_created,rolling_usage,time_rolling_updated,{columns} FROM `{table}` WHERE workspace_id=? AND user_id=? AND time_deleted IS NULL LIMIT 1"), (workspace, user)).await?;
    row.map(|row| {
        Ok(Counters {
            created: row
                .get(0)
                .ok_or_else(|| UsageError::internal("missing entitlement creation timestamp"))?,
            rolling: row.get::<Option<i64>, _>(1).flatten(),
            rolling_at: row.get::<Option<PrimitiveDateTime>, _>(2).flatten(),
            weekly: row.get::<Option<i64>, _>(3).flatten(),
            monthly: row.get::<Option<i64>, _>(4).flatten(),
            weekly_at: row.get::<Option<PrimitiveDateTime>, _>(5).flatten(),
            monthly_at: row.get::<Option<PrimitiveDateTime>, _>(6).flatten(),
        })
    })
    .transpose()
}
