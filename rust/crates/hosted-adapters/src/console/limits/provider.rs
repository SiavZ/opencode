//! Purpose: enforce provider per-minute budgets and deterministic legacy provider placement.
//! Scope: Redis priority budget snapshots and selection over normalized model providers.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/limits.rs
//! `-- hosted-adapters/src/console/limits/store.rs
//! External:
//! |-- rust_decimal::Decimal
//! |-- serde_json::Value
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: priority one always qualifies; sticky selection yields only to budget or TPS preference.
//! Contract tests: hosted-adapters/tests/console_gateway_limits.rs

use super::{LimitError, RedisCounters, interval, store::RoutingSnapshot};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Default)]
pub struct BudgetSnapshot {
    limits: HashMap<String, (i64, i64, i64, i64)>,
}

impl BudgetSnapshot {
    pub fn qualify(&self, id: &str, priority: i64) -> bool {
        priority <= 1
            || self
                .limits
                .get(&format!("{id}/{priority}"))
                .is_some_and(|v| v.0 < v.1)
    }
    pub fn prefer(&self, id: &str, priority: i64) -> bool {
        self.limits
            .get(&format!("{id}/{priority}"))
            .is_some_and(|v| i128::from(v.2) * 10 < i128::from(v.3) * 8)
    }
}

pub struct ProviderBudget {
    redis: RedisCounters,
    providers: Vec<Value>,
    minute: String,
    previous: String,
}
impl ProviderBudget {
    pub fn new(redis: RedisCounters, providers: &[Value], now: OffsetDateTime) -> Option<Self> {
        let providers = providers
            .iter()
            .filter(|p| {
                p.get("budget").is_some()
                    && p.get("budgetContribution").is_some()
                    && p.get("budgetPriority").is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        (!providers.is_empty()).then(|| Self {
            redis,
            providers,
            minute: interval(now),
            previous: interval(now - time::Duration::minutes(1)),
        })
    }

    pub async fn snapshot(&self) -> Result<BudgetSnapshot, LimitError> {
        let mut groups: HashMap<String, (i64, i64)> = HashMap::new();
        for provider in &self.providers {
            let id = provider["id"]
                .as_str()
                .ok_or(LimitError::Backend)?
                .to_owned();
            let budget = rounded_microcents(
                provider["budget"].as_f64().ok_or(LimitError::Backend)? * 100.0,
            )?;
            let priority = provider["budgetPriority"]
                .as_i64()
                .ok_or(LimitError::Backend)?;
            let group = groups.entry(id).or_insert((budget, 0));
            group.1 = group.1.max(priority);
        }
        let mut result = BudgetSnapshot::default();
        for (id, (budget, maximum)) in groups {
            let keys = (1..=maximum)
                .flat_map(|priority| {
                    [
                        self.redis.key(
                            "provider-budget",
                            &format!("{id}:{priority}"),
                            Some(&self.minute),
                        ),
                        self.redis.key(
                            "provider-budget",
                            &format!("{id}:{priority}"),
                            Some(&self.previous),
                        ),
                    ]
                })
                .collect::<Vec<_>>();
            let values = self.redis.values(&keys).await?;
            let mut current = 0;
            let mut previous = 0;
            for priority in 1..=maximum {
                let offset = (priority as usize - 1) * 2;
                current += values[offset];
                let effective = (budget - previous).max(0);
                previous += values[offset + 1];
                result.limits.insert(
                    format!("{id}/{priority}"),
                    (current, effective, previous, budget),
                );
            }
        }
        Ok(result)
    }

    pub async fn track(&self, provider: &Value, cost_in_cents: f64) -> Result<(), LimitError> {
        let Some(config) = self.providers.iter().find(|p| {
            p["id"] == provider["id"] && p["budgetPriority"] == provider["budgetPriority"]
        }) else {
            return Ok(());
        };
        let cost = rounded_microcents(
            cost_in_cents
                * config["budgetContribution"]
                    .as_f64()
                    .ok_or(LimitError::Backend)?,
        )?;
        if cost <= 0 {
            return Ok(());
        }
        let key = self.redis.key(
            "provider-budget",
            &format!(
                "{}:{}",
                config["id"].as_str().ok_or(LimitError::Backend)?,
                config["budgetPriority"]
            ),
            Some(&self.minute),
        );
        self.redis
            .commands(vec![
                json!(["INCRBY", key, cost]),
                json!(["EXPIRE", key, 120]),
            ])
            .await?;
        Ok(())
    }
}

fn rounded_microcents(cents: f64) -> Result<i64, LimitError> {
    Decimal::from_f64_retain(cents * 1_000_000.0)
        .and_then(|value| value.checked_add(Decimal::new(5, 1)))
        .and_then(|value| value.floor().to_i64())
        .ok_or(LimitError::Backend)
}

pub fn select<'a>(
    model: &'a Value,
    sticky_id: &str,
    byok: bool,
    counters: &RoutingSnapshot,
    budget: Option<&BudgetSnapshot>,
) -> Option<&'a Value> {
    let providers = model.get("providers")?.as_array()?;
    if byok {
        return providers.iter().find(|p| p["id"] == model["byokProvider"]);
    }
    let enabled = providers
        .iter()
        .filter(|p| p["disabled"] != true)
        .collect::<Vec<_>>();
    let fallback = enabled
        .iter()
        .find(|p| p["id"] == model["fallbackProvider"])
        .copied();
    let qualifies = enabled
        .iter()
        .copied()
        .filter(|provider| {
            if provider.get("weight").and_then(Value::as_u64) == Some(0) {
                return false;
            }
            let id = provider["id"].as_str().unwrap_or_default();
            if let Some(priority) = provider.get("budgetPriority").and_then(Value::as_i64)
                && budget.is_some_and(|b| !b.qualify(id, priority))
            {
                return false;
            }
            if let Some(limit) = provider
                .get("tpmLimit")
                .and_then(Value::as_f64)
                .filter(|v| *v != 0.0)
                && counters
                    .tpm
                    .get(&format!(
                        "{id}/{}",
                        provider["model"].as_str().unwrap_or_default()
                    ))
                    .copied()
                    .unwrap_or_default() as f64
                    >= limit * 1_000_000.0
            {
                return false;
            }
            let (qualify, unqualify) = tps(provider, counters);
            qualify + unqualify <= 10 || qualify >= unqualify
        })
        .collect::<Vec<_>>();
    let priority = |p: &&Value| {
        if counters.trial
            && model["trialProvider"]
                .as_array()
                .is_some_and(|trials| trials.contains(&p["id"]))
        {
            0.0
        } else {
            p["priority"].as_f64().unwrap_or(f64::INFINITY)
        }
    };
    let minimum = qualifies.iter().map(priority).fold(f64::INFINITY, f64::min);
    let best = qualifies
        .iter()
        .copied()
        .filter(|p| priority(p) <= minimum)
        .collect::<Vec<_>>();
    let total = best
        .iter()
        .map(|p| p["weight"].as_u64().unwrap_or(1))
        .sum::<u64>();
    let utf16 = sticky_id.encode_utf16().collect::<Vec<_>>();
    // JS charCodeAt on a negative index yields NaN, which bitwise coercion turns into zero.
    let hash = (utf16.len() as isize - 4..utf16.len() as isize).fold(0u32, |hash, i| {
        if i < 0 {
            0
        } else {
            hash.wrapping_mul(31)
                .wrapping_add(u32::from(utf16[i as usize]))
        }
    });
    let mut index = if total > 0 {
        u64::from(hash) % total
    } else {
        0
    };
    let selected = best
        .iter()
        .copied()
        .find(|p| {
            let weight = p["weight"].as_u64().unwrap_or(1);
            if index < weight {
                return true;
            }
            index -= weight;
            false
        })
        .or(fallback)?;
    let sticky = enabled
        .iter()
        .find(|p| Some(p["id"].as_str().unwrap_or_default()) == counters.sticky.as_deref())
        .copied();
    let prefer_budget = selected["budgetPriority"].as_i64().is_some_and(|p| {
        budget.is_some_and(|b| b.prefer(selected["id"].as_str().unwrap_or_default(), p))
    });
    let (qualify, unqualify) = tps(selected, counters);
    if !prefer_budget && qualify <= unqualify * 3 {
        return sticky.or(Some(selected));
    }
    Some(selected)
}

fn tps(provider: &Value, counters: &RoutingSnapshot) -> (i64, i64) {
    counters
        .tps
        .get(&format!(
            "{}/{}/{}",
            provider["id"].as_str().unwrap_or_default(),
            provider["model"].as_str().unwrap_or_default(),
            provider["tpsGoal"]
        ))
        .copied()
        .unwrap_or_default()
}
