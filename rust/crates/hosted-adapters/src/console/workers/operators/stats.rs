//! Purpose: produce baseline Black per-seat ISO-week statistics CSV from durable usage.
//! Scope: local read-only reporting adapter.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/workers.rs
//! `-- hosted-adapters/src/console/workers/operators.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::{Value,json}
//! `-- time::{Date,Month,PrimitiveDateTime}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators.rs
//! Invariants: subscriber cutoff is 2026-01-31T23:59:59.999Z; usage itself is not cutoff-filtered.
//! Contract tests: hosted-adapters/tests/console_workers_operators.rs

use super::field;
use crate::console::{db::ConsoleDb, workers::WorkerError};
use mysql_async::prelude::Queryable;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use time::{Date, PrimitiveDateTime};

fn fixed(value: f64, digits: u32) -> String {
    // Number.toFixed rounds the exact binary value, including decimal half cases.
    let bits = value.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let significand =
        u128::from((bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u64 << 52 });
    let shift = if exponent == 0 {
        -1074
    } else {
        exponent - 1023 - 52
    };
    let numerator = significand * 10_u128.pow(digits);
    let rounded = if shift >= 0 {
        numerator.checked_shl(shift as u32).unwrap_or(u128::MAX)
    } else if -shift >= 128 {
        0
    } else {
        let shift = (-shift) as u32;
        let whole = numerator >> shift;
        whole + u128::from((numerator & ((1_u128 << shift) - 1)) >= (1_u128 << (shift - 1)))
    };
    let scale = 10_u128.pow(digits);
    format!(
        "{}{}.{:0width$}",
        if value < 0.0 { "-" } else { "" },
        rounded / scale,
        rounded % scale,
        width = digits as usize
    )
}

fn money(value: f64) -> String {
    format!("${}", fixed(value / 100_000_000.0, 2))
}
fn csv(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.into()
    }
}
fn week_start(week: i32) -> Result<Date, WorkerError> {
    Date::from_iso_week_date(week / 100, (week % 100) as u8, time::Weekday::Monday)
        .map_err(|e| WorkerError::Validation(e.to_string()))
}
fn date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

pub(super) async fn run(input: &Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    let plan = field(input, "plan")?;
    if !["20", "100", "200"].contains(&plan) {
        return Err(WorkerError::Validation("Invalid Black plan".into()));
    }
    let mut conn = db.conn().await?;
    let ids:Vec<String>=conn.exec("SELECT workspace_id FROM billing WHERE subscription_id IS NOT NULL AND JSON_UNQUOTE(JSON_EXTRACT(subscription,'$.plan')) = ?",(plan,)).await?;
    if ids.is_empty() {
        return Err(WorkerError::Validation(format!(
            "No active Black {plan} subscriptions found"
        )));
    }
    let mut weeks = BTreeSet::new();
    let mut spend = HashMap::new();
    let mut ppu = HashMap::new();
    let mut models = HashMap::new();
    let mut tokens = HashMap::new();
    let mut users = Vec::new();
    for id in &ids {
        let sub:Vec<(i32,f64)>=conn.exec("SELECT YEARWEEK(time_created,3),CAST(COALESCE(SUM(cost),0) AS DOUBLE) FROM `usage` WHERE workspace_id = ? AND JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) = 'sub' GROUP BY YEARWEEK(time_created,3)",(id,)).await?;
        let pay:Vec<(i32,f64)>=conn.exec("SELECT YEARWEEK(time_created,3),CAST(COALESCE(SUM(cost),0) AS DOUBLE) FROM `usage` WHERE workspace_id = ? AND (enrichment IS NULL OR JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) != 'sub') GROUP BY YEARWEEK(time_created,3)",(id,)).await?;
        for (week, _) in sub.iter().chain(pay.iter()) {
            weeks.insert(*week);
        }
        spend.insert(id.clone(), sub.into_iter().collect::<BTreeMap<_, _>>());
        ppu.insert(id.clone(), pay.into_iter().collect::<BTreeMap<_, _>>());
        let mut model:Vec<(String,f64)>=conn.exec("SELECT model,CAST(COALESCE(SUM(cost),0) AS DOUBLE) FROM `usage` WHERE workspace_id = ? AND JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) = 'sub' GROUP BY model",(id,)).await?;
        model.sort_by(|a, b| b.1.total_cmp(&a.1));
        models.insert(id.clone(), model);
        let token:Vec<(i32,f64,f64,f64)>=conn.exec("SELECT YEARWEEK(time_created,3),CAST(COALESCE(SUM(input_tokens),0) AS DOUBLE),CAST(COALESCE(SUM(cache_read_tokens),0) AS DOUBLE),CAST(COALESCE(SUM(output_tokens),0)+COALESCE(SUM(reasoning_tokens),0) AS DOUBLE) FROM `usage` WHERE workspace_id = ? AND JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) = 'sub' GROUP BY YEARWEEK(time_created,3)",(id,)).await?;
        tokens.insert(
            id.clone(),
            token
                .into_iter()
                .map(|(week, input, cache, output)| (week, (input, cache, output)))
                .collect::<BTreeMap<_, _>>(),
        );
        let seats:Vec<(PrimitiveDateTime,Option<String>)>=conn.exec("SELECT s.time_created,CAST(b.subscription AS CHAR) FROM subscription s INNER JOIN billing b ON b.workspace_id = s.workspace_id WHERE s.workspace_id = ? AND s.time_created <= '2026-01-31 23:59:59.999'",(id,)).await?;
        let count = seats.len() as f64;
        for (subscribed, subscription) in seats {
            users.push((id.clone(), subscribed, subscription, count));
        }
    }
    users.sort_by_key(|user| user.1);
    let mut header = [
        "workspaceID",
        "subscribed",
        "useCredit",
        "subTotal",
        "ppuTotal",
        "model1",
        "model1%",
        "model2",
        "model2%",
        "model3",
        "model3%",
    ]
    .map(str::to_string)
    .to_vec();
    let weeks = weeks
        .into_iter()
        .map(|week| Ok((week, week_start(week)?)))
        .collect::<Result<Vec<_>, WorkerError>>()?;
    for (_, week) in &weeks {
        for label in ["sub", "ppu", "input", "cache", "output"] {
            header.push(format!("{} {label}", date(*week)));
        }
    }
    let mut lines = vec![header.iter().map(|v| csv(v)).collect::<Vec<_>>().join(",")];
    let mut total = 0.0;
    for (id, subscribed, subscription, count) in &users {
        let spend = &spend[id];
        let ppu = &ppu[id];
        let model_total = spend.values().sum::<f64>();
        let amount = model_total / count;
        total += amount;
        let subscription: Value = subscription
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or(Value::Null);
        let mut cells = vec![
            id.clone(),
            date(subscribed.date()),
            if subscription
                .get("useBalance")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "yes".into()
            } else {
                "no".into()
            },
            money(amount),
            money(ppu.values().sum::<f64>() / count),
        ];
        for index in 0..3 {
            if let Some((model, cost)) = models[id].get(index) {
                cells.push(model.clone());
                cells.push(if model_total > 0.0 {
                    format!("{}%", fixed(cost / model_total * 100.0, 1))
                } else {
                    "0.0%".into()
                });
            } else {
                cells.extend(["-".into(), "-".into()]);
            }
        }
        let month = Date::from_calendar_date(subscribed.year(), subscribed.month(), 1)
            .map_err(|e| WorkerError::Validation(e.to_string()))?;
        for (week, start) in &weeks {
            if *start < month {
                cells.extend(std::iter::repeat_n("-".into(), 5));
                continue;
            }
            cells.extend([
                money(spend.get(week).copied().unwrap_or_default() / count),
                money(ppu.get(week).copied().unwrap_or_default() / count),
            ]);
            let (input, cache, output) = tokens[id].get(week).copied().unwrap_or_default();
            cells.extend(
                [input, cache, output].map(|v| format!("{:.0}", (v / count + 0.5).floor())),
            );
        }
        lines.push(cells.iter().map(|v| csv(v)).collect::<Vec<_>>().join(","));
    }
    let output = format!("{}\n", lines.join("\n"));
    Ok(
        json!({"filename":format!("black-stats-{plan}.csv"),"csv":output,"subscribers":users.len(),"average":money(if users.is_empty(){0.0}else{total/users.len() as f64})}),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn console_workers_stats_binary_rounding_and_csv_edges() {
        assert_eq!(super::fixed(1.125, 2), "1.13");
        assert_eq!(super::fixed(1.005, 2), "1.00");
        assert_eq!(super::fixed(-1.125, 2), "-1.13");
        assert_eq!(super::fixed(0.0, 2), "0.00");
        assert_eq!(
            super::csv("model,\"quoted\"\n"),
            "\"model,\"\"quoted\"\"\n\""
        );
        assert_eq!(
            super::date(super::week_start(202601).unwrap()),
            "2025-12-29"
        );
    }
}
