//! Purpose: reproduce legacy token pricing and subscription quota windows.
//! Scope: pure microcent pricing, China peak hours, UTC week and anchored month calculations.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-domain/src/console/quotas.rs
//! External:
//! |-- rust_decimal::Decimal
//! |-- serde_json::Value
//! `-- time::{Duration, PrimitiveDateTime}
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/gateway/usage/auth.rs
//! `-- hosted-adapters/src/console/gateway/usage/record.rs
//! Invariants: reasoning is already included in output; tier threshold is strict and includes cache tokens.
//! Contract tests: hosted-adapters/tests/console_gateway_usage.rs

pub(super) use opencode_hosted_domain::console::quotas::{analyze, month, rolling, week};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use time::{Duration, PrimitiveDateTime};

use super::UsageError;
use crate::console::formats::UsageInfo;

pub fn calculate_cost(
    model: &Value,
    usage: &UsageInfo,
    at: PrimitiveDateTime,
) -> Result<i64, UsageError> {
    calculate_cost_decimal(model, usage, at).and_then(|cost| {
        round_money(cost)
            .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Usage cost overflow"))
    })
}

pub fn calculate_cost_decimal(
    model: &Value,
    usage: &UsageInfo,
    at: PrimitiveDateTime,
) -> Result<Decimal, UsageError> {
    let cents = calculate_cost_in_cents(model, usage, at)?;
    Decimal::from_f64_retain(cents * 1_000_000.0)
        .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Usage cost overflow"))
}

pub fn calculate_cost_in_cents(
    model: &Value,
    usage: &UsageInfo,
    at: PrimitiveDateTime,
) -> Result<f64, UsageError> {
    let prices = selected_prices(model, usage, at)?;
    cost_in_cents(prices, usage)
}

fn selected_prices<'a>(
    model: &'a Value,
    usage: &UsageInfo,
    at: PrimitiveDateTime,
) -> Result<&'a Value, UsageError> {
    let china = at + Duration::hours(8);
    let peak = china.weekday().number_days_from_monday() < 5
        && ((9..12).contains(&china.hour()) || (14..18).contains(&china.hour()));
    let context = [
        usage.input_tokens,
        usage.cache_read_tokens.unwrap_or_default(),
        usage.cache_write5m_tokens.unwrap_or_default(),
        usage.cache_write1h_tokens.unwrap_or_default(),
    ]
    .into_iter()
    .try_fold(0i64, |sum, value| sum.checked_add(value))
    .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Token count overflow"))?;
    let prices = if peak && model["costPeak"].is_object() {
        &model["costPeak"]
    } else if model["cost200K"].is_object()
        && context > model["cost200K"]["threshold"].as_i64().unwrap_or(200_000)
    {
        &model["cost200K"]
    } else {
        &model["cost"]
    };
    Ok(prices)
}

fn cost_in_cents(prices: &Value, usage: &UsageInfo) -> Result<f64, UsageError> {
    // Keep JavaScript Number operation order. Decimal-only arithmetic changes half-microcent ties.
    let cents = [
        ("input", Some(usage.input_tokens)),
        ("output", Some(usage.output_tokens)),
        ("cacheRead", usage.cache_read_tokens),
        ("cacheWrite5m", usage.cache_write5m_tokens),
        ("cacheWrite1h", usage.cache_write1h_tokens),
    ]
    .into_iter()
    .try_fold(0.0, |sum, (name, tokens)| {
        let Some(tokens) = tokens else {
            return Ok(sum);
        };
        let price = if prices[name].is_null() && name != "input" && name != "output" {
            0.0
        } else {
            prices[name]
                .as_f64()
                .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Invalid model price"))?
        };
        Ok::<_, UsageError>(sum + price * tokens as f64 * 100.0)
    })?;
    if !cents.is_finite() {
        return Err(UsageError::new(
            500,
            "ConfigurationError",
            "Usage cost overflow",
        ));
    }
    Ok(cents)
}

/// Returns legacy logger cost metric fields using one full-request price tier.
pub fn calculate_cost_components(
    model: &Value,
    usage: &UsageInfo,
    at: PrimitiveDateTime,
) -> Result<Value, UsageError> {
    let prices = selected_prices(model, usage, at)?;
    let total = cost_in_cents(prices, usage)?;
    let rounded = |value: f64| -> Result<i64, UsageError> {
        Decimal::from_f64_retain(value)
            .and_then(round_money)
            .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Usage cost overflow"))
    };
    let mut metrics = json!({});
    for (price_name, metric_name, tokens) in [
        ("input", "input", Some(usage.input_tokens)),
        ("output", "output", Some(usage.output_tokens)),
        ("cacheRead", "cache_read", usage.cache_read_tokens),
        ("cacheWrite5m", "cache_write_5m", usage.cache_write5m_tokens),
        ("cacheWrite1h", "cache_write_1h", usage.cache_write1h_tokens),
    ] {
        let required = matches!(price_name, "input" | "output");
        let Some(tokens) = tokens else {
            continue;
        };
        let price = prices[price_name].as_f64();
        if !required && (tokens == 0 || price.is_none_or(|price| price == 0.0)) {
            continue;
        }
        let cents = price
            .ok_or_else(|| UsageError::new(500, "ConfigurationError", "Invalid model price"))?
            * tokens as f64
            * 100.0;
        metrics[format!("cost.{metric_name}")] = json!(rounded(cents)?);
        // The legacy logger calls the five-minute write field cache_write and has no one-hour microcent field.
        if metric_name != "cache_write_1h" {
            let micro_name = if metric_name == "cache_write_5m" {
                "cache_write"
            } else {
                metric_name
            };
            metrics[format!("cost.{micro_name}.microcents")] = json!(rounded(cents * 1_000_000.0)?);
        }
    }
    metrics["cost.total"] = json!(rounded(total)?);
    metrics["cost.total.microcents"] = json!(rounded(total * 1_000_000.0)?);
    Ok(metrics)
}

// JavaScript Math.round ties toward positive infinity, including negative half-microcents.
pub(super) fn round_money(value: Decimal) -> Option<i64> {
    value
        .checked_add(Decimal::new(5, 1))
        .and_then(|value| value.floor().to_i64())
}

pub(super) fn limit(value: &Value, name: &str) -> Result<i64, UsageError> {
    value[name]
        .as_i64()
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            UsageError::new(
                500,
                "ConfigurationError",
                format!("Missing or invalid subscription limit {name}"),
            )
        })
}

pub(super) fn retry_time(seconds: i64) -> String {
    let days = seconds / 86400;
    if days >= 1 {
        return format!("{days} day{}", if days > 1 { "s" } else { "" });
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600 + 59) / 60;
    if hours >= 1 {
        return format!("{hours}hr {minutes}min");
    }
    format!("{minutes}min")
}
