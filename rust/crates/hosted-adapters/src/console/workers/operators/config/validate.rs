//! Purpose: validate model and limit config shapes used by baseline operator scripts.
//! Scope: pure JSON validation before configuration store writes.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators/config.rs
//! Invariants: validation preserves original JSON; unknown keys are not discarded during secret updates.
//! Contract tests: hosted-adapters/tests/console_workers_config.rs

use crate::console::workers::WorkerError;
use serde_json::Value;

fn check(ok: bool, path: &str) -> Result<(), WorkerError> {
    if ok {
        Ok(())
    } else {
        Err(WorkerError::Validation(format!("Invalid config: {path}")))
    }
}
fn object<'a>(v: &'a Value, path: &str) -> Result<&'a serde_json::Map<String, Value>, WorkerError> {
    v.as_object()
        .ok_or_else(|| WorkerError::Validation(format!("Invalid config object: {path}")))
}
fn fields(v: &Value, names: &[&str], predicate: fn(&Value) -> bool) -> Result<(), WorkerError> {
    for name in names {
        check(v.get(name).is_some_and(predicate), name)?;
    }
    Ok(())
}
fn optional(v: &Value, names: &[&str], predicate: fn(&Value) -> bool) -> Result<(), WorkerError> {
    for name in names {
        if let Some(v) = v.get(name) {
            check(predicate(v), name)?;
        }
    }
    Ok(())
}
fn number(v: &Value) -> bool {
    v.is_number()
}
fn integer(v: &Value) -> bool {
    v.as_f64().is_some_and(|n| n.fract() == 0.0)
}
fn string(v: &Value) -> bool {
    v.is_string()
}
fn boolean(v: &Value) -> bool {
    v.is_boolean()
}
fn format(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| ["anthropic", "google", "openai", "oa-compat", "systemone"].contains(&s))
}
fn cost(v: &Value) -> Result<(), WorkerError> {
    object(v, "cost")?;
    fields(v, &["input", "output"], number)?;
    optional(v, &["cacheRead", "cacheWrite5m", "cacheWrite1h"], number)
}

pub(super) fn models(v: &Value) -> Result<(), WorkerError> {
    object(v, "models")?;
    for group in ["zenModels", "liteModels"] {
        let models = object(&v[group], group)?;
        for model in models.values() {
            let models = model
                .as_array()
                .map(|v| v.iter().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![model]);
            for variant in models {
                object(variant, "model")?;
                if model.is_array() {
                    check(format(&variant["formatFilter"]), "formatFilter")?;
                }
                fields(variant, &["name"], string)?;
                cost(&variant["cost"])?;
                optional(variant, &["costMultiplier", "rateLimit"], number)?;
                optional(variant, &["allowAnonymous", "trialEnded"], boolean)?;
                optional(variant, &["trialProvider", "fallbackProvider"], string)?;
                if let Some(value) = variant.get("byokProvider") {
                    check(
                        value
                            .as_str()
                            .is_some_and(|v| ["openai", "anthropic", "google"].contains(&v)),
                        "byokProvider",
                    )?;
                }
                if let Some(value) = variant.get("stickyProvider") {
                    check(
                        value
                            .as_str()
                            .is_some_and(|v| ["strict", "prefer"].contains(&v)),
                        "stickyProvider",
                    )?;
                }
                for name in ["cost200K", "costPeak"] {
                    if let Some(value) = variant.get(name) {
                        cost(value)?;
                        if name == "cost200K" {
                            optional(value, &["threshold"], number)?;
                        }
                    }
                }
                let providers = variant["providers"]
                    .as_array()
                    .ok_or_else(|| WorkerError::Validation("Invalid providers array".into()))?;
                for provider in providers {
                    object(provider, "model provider")?;
                    fields(provider, &["id", "model"], string)?;
                    optional(
                        provider,
                        &[
                            "priority",
                            "tpmLimit",
                            "tpsGoal",
                            "budgetPriority",
                            "budgetContribution",
                            "weight",
                        ],
                        number,
                    )?;
                    optional(provider, &["disabled"], boolean)?;
                    optional(provider, &["storeModel"], string)?;
                    if let Some(payload) = provider.get("payloadModifier") {
                        object(payload, "payloadModifier")?;
                    }
                }
            }
        }
    }
    for provider in object(&v["providers"], "providers")?.values() {
        object(provider, "provider")?;
        fields(provider, &["api"], string)?;
        optional(provider, &["displayName"], string)?;
        let key = &provider["apiKey"];
        check(
            key.is_string()
                || key
                    .as_object()
                    .is_some_and(|v| v.values().all(Value::is_string)),
            "apiKey",
        )?;
        optional(provider, &["format"], format)?;
        optional(provider, &["budget"], number)?;
        optional(provider, &["adjustCacheUsage"], boolean)?;
        for name in ["headerModifier", "payloadModifier"] {
            if let Some(value) = provider.get(name) {
                object(value, name)?;
            }
        }
    }
    Ok(())
}

pub(super) fn limits(v: &Value) -> Result<(), WorkerError> {
    object(v, "limits")?;
    fields(
        &v["free"],
        &["promoTokens", "dailyRequests", "dailyRequestsFallback"],
        integer,
    )?;
    check(
        object(&v["free"]["checkHeaders"], "checkHeaders")?
            .values()
            .all(Value::is_string),
        "checkHeaders",
    )?;
    fields(
        &v["lite"],
        &[
            "rollingLimit",
            "rollingWindow",
            "weeklyLimit",
            "monthlyLimit",
        ],
        integer,
    )?;
    for plan in ["20", "100", "200"] {
        fields(
            &v["black"][plan],
            &["fixedLimit", "rollingLimit", "rollingWindow"],
            integer,
        )?;
    }
    Ok(())
}
