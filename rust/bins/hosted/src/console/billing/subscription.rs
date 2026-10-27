//! Purpose: reproduce baseline Black and Lite subscription UI query results.
//! Scope: subscription windows, balance preferences and grouped Lite model contributions.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/billing.rs
//! `-- hosted/src/console/zen.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/quotas.rs
//! External:
//! |-- serde_json::{Value, json}
//! `-- time::{Duration, PrimitiveDateTime}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing/operations.rs
//! Invariants: only subscription owner sees Lite breakdown; expired counters contribute zero.
//! Contract tests: hosted/src/console/billing/lifecycle_tests.rs

use super::*;
use opencode_hosted_domain::console::quotas;
use serde_json::{Value, json};
use time::{Duration, PrimitiveDateTime};

pub(super) async fn query(
    state: &ConsoleState,
    workspace: &str,
    user: &str,
    lite: bool,
    window: Option<&str>,
) -> Result<Value, Response> {
    let store = BillingStore::new(state.db.clone());
    let Some(row) = store
        .subscription_usage(workspace, lite)
        .await
        .map_err(store_error)?
    else {
        return Ok(Value::Null);
    };
    if !lite && row.settings.is_none() {
        return Ok(Value::Null);
    }
    let configured = state.config.zen.billing_limits().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Subscription limits are not configured",
        )
    })?;
    let plan = row
        .settings
        .as_ref()
        .and_then(|value| value["plan"].as_str())
        .unwrap_or("20");
    let limits = if lite {
        &configured["lite"]
    } else {
        &configured["black"][plan]
    };
    let limit = |key: &str| {
        limits[key]
            .as_i64()
            .filter(|value| *value >= 0)
            .ok_or_else(|| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Subscription limit is invalid",
                )
            })
    };
    let now = time::OffsetDateTime::now_utc();
    let now = PrimitiveDateTime::new(now.date(), now.time());
    let week = quotas::week(now);
    let month = quotas::month(now, row.time_created);
    let hours = limit("rollingWindow")?;
    let rolling_limit = limit("rollingLimit")?;
    let weekly_limit = limit(if lite { "weeklyLimit" } else { "fixedLimit" })?;
    let rolling = quotas::rolling(
        rolling_limit,
        hours,
        row.rolling_usage,
        row.time_rolling_updated,
        now,
    );
    let weekly = quotas::analyze(
        weekly_limit,
        row.weekly_usage,
        row.time_weekly_updated.or(Some(now)),
        week.0,
        week.1,
        now,
    );
    if !lite {
        return Ok(
            json!({"plan":plan,"useBalance":row.settings.as_ref().and_then(|value| value["useBalance"].as_bool()).unwrap_or(false),"rollingUsage":rolling.summary(),"weeklyUsage":weekly.summary()}),
        );
    }
    let monthly_limit = limit("monthlyLimit")?;
    let monthly = quotas::analyze(
        monthly_limit,
        row.monthly_usage,
        row.time_monthly_updated.or(Some(now)),
        month.0,
        month.1,
        now,
    );
    let active =
        |updated: Option<PrimitiveDateTime>, start: PrimitiveDateTime, usage: Option<i64>| {
            if updated.is_some_and(|updated| updated >= start) {
                usage.unwrap_or_default()
            } else {
                0
            }
        };
    let rolling_usage = active(
        row.time_rolling_updated,
        now - Duration::hours(hours),
        row.rolling_usage,
    );
    let weekly_usage = active(row.time_weekly_updated, week.0, row.weekly_usage);
    let monthly_usage = active(row.time_monthly_updated, month.0, row.monthly_usage);
    if let Some(window) = window {
        if row.user_id != user {
            return Ok(Value::Null);
        }
        let (start, usage, limit) = match window {
            "rolling" => (
                if row
                    .time_rolling_updated
                    .is_some_and(|updated| updated >= now - Duration::hours(hours))
                {
                    row.time_rolling_updated.unwrap()
                } else {
                    now
                },
                rolling_usage,
                rolling_limit,
            ),
            "weekly" => (week.0, weekly_usage, weekly_limit),
            "monthly" => (month.0, monthly_usage, monthly_limit),
            _ => return Ok(Value::Null),
        };
        return breakdown(
            state,
            &store,
            workspace,
            user,
            start,
            usage,
            limit * 100_000_000,
        )
        .await;
    }
    let enrich = |mut summary: Value, usage: i64, limit: i64| {
        summary["usage"] = usage.into();
        summary["limit"] = (limit * 100_000_000).into();
        summary["usagePercent"] = usage_percent(usage as f64, (limit * 100_000_000) as f64).into();
        summary
    };
    Ok(
        json!({"mine":row.user_id==user,"useBalance":row.settings.as_ref().and_then(|value| value["useBalance"].as_bool()).unwrap_or(false),"allowTraining":row.allow_training,"region":row.region,"rollingUsage":enrich(rolling.summary(),rolling_usage,rolling_limit),"weeklyUsage":enrich(weekly.summary(),weekly_usage,weekly_limit),"monthlyUsage":enrich(monthly.summary(),monthly_usage,monthly_limit)}),
    )
}

fn usage_percent(usage: f64, limit: f64) -> f64 {
    if limit == 0.0 {
        return 0.0;
    }
    (usage / limit * 1000.0).round() / 10.0
}

async fn breakdown(
    state: &ConsoleState,
    store: &BillingStore,
    workspace: &str,
    user: &str,
    start: PrimitiveDateTime,
    usage: i64,
    limit: i64,
) -> Result<Value, Response> {
    let sources = store
        .lite_usage_sources(workspace, user, start)
        .await
        .map_err(store_error)?;
    let models = state.config.zen.billing_lite_models().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Lite model catalog is not configured",
        )
    })?;
    let mut groups = Vec::<Value>::new();
    for (model, multiplier, cost, quota) in sources {
        let info = models.get(&model);
        let candidates = info
            .map(|value| {
                value
                    .as_array()
                    .map(|values| values.iter().collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![value])
            })
            .unwrap_or_default();
        let name = candidates
            .first()
            .and_then(|value| value["name"].as_str())
            .unwrap_or(&model);
        let configured = candidates
            .first()
            .and_then(|value| value["costMultiplier"].as_f64())
            .filter(|first| {
                candidates
                    .iter()
                    .all(|value| value["costMultiplier"].as_f64() == Some(*first))
            });
        let estimated = multiplier.is_none();
        let multiplier = multiplier
            .map(|value| value.parse::<f64>())
            .transpose()
            .map_err(store_error)?
            .or(configured);
        let cost = cost.parse::<f64>().map_err(store_error)?;
        let quota = if estimated {
            (cost * multiplier.unwrap_or(1.0)).round()
        } else {
            quota.parse::<f64>().map_err(store_error)?
        };
        if let Some(existing) = groups
            .iter_mut()
            .find(|value| value["model"] == model && value["multiplier"].as_f64() == multiplier)
        {
            existing["cost"] = (existing["cost"].as_f64().unwrap_or_default() + cost).into();
            existing["quotaCost"] =
                (existing["quotaCost"].as_f64().unwrap_or_default() + quota).into();
            existing["estimated"] =
                (existing["estimated"].as_bool().unwrap_or_default() || estimated).into();
        } else {
            let mut value = json!({"model":model,"name":name,"cost":cost,"quotaCost":quota,"estimated":estimated,"contributionPercent":0});
            if let Some(multiplier) = multiplier {
                value["multiplier"] = multiplier.into();
            }
            groups.push(value);
        }
    }
    groups.retain(|value| {
        value["cost"].as_f64() != Some(0.0) || value["quotaCost"].as_f64() != Some(0.0)
    });
    groups.sort_by(|a, b| {
        b["quotaCost"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&a["quotaCost"].as_f64().unwrap_or_default())
    });
    let percent = usage_percent(usage as f64, limit as f64);
    let target = (percent * 10.0).round().max(0.0) as i64;
    let total: f64 = groups
        .iter()
        .map(|value| value["quotaCost"].as_f64().unwrap_or_default().max(0.0))
        .sum();
    let units = groups
        .iter()
        .map(|value| {
            if total == 0.0 {
                0.0
            } else {
                value["quotaCost"].as_f64().unwrap_or_default().max(0.0) / total * target as f64
            }
        })
        .collect::<Vec<_>>();
    let mut allocated = units
        .iter()
        .map(|value| value.floor() as i64)
        .collect::<Vec<_>>();
    let mut ranked = (0..units.len()).collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        (units[*b] - units[*b].floor()).total_cmp(&(units[*a] - units[*a].floor()))
    });
    if !ranked.is_empty() {
        for index in 0..(target - allocated.iter().sum::<i64>()).max(0) as usize {
            allocated[ranked[index % ranked.len()]] += 1;
        }
    }
    for (value, units) in groups.iter_mut().zip(allocated) {
        value["contributionPercent"] = (units as f64 / 10.0).into();
    }
    Ok(json!({"usage":usage,"limit":limit,"usagePercent":percent,"rows":groups}))
}
