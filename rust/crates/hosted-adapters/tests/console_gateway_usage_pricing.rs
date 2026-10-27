//! Purpose: verify legacy native usage pricing at money and timing boundaries.
//! Scope: pure public cost calculation, no I/O.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! External:
//! |-- serde_json::json
//! `-- time::{Date, Month}
//! Used by:
//! `-- none, entrypoint
//! Invariants: cost counts reasoning once, cache tokens select tiers and negative ties round toward positive infinity.
//! Contract tests: this file.

use opencode_hosted_adapters::console::formats::UsageInfo;
use opencode_hosted_adapters::console::gateway::usage::calculate_cost;
use opencode_hosted_adapters::console::gateway::usage::calculate_cost_components;
use serde_json::json;
use time::{Date, Month};

#[test]
fn usage_error_stays_within_small_result_size() {
    assert!(
        std::mem::size_of::<opencode_hosted_adapters::console::gateway::usage::UsageError>() <= 128
    );
}

#[test]
fn component_metrics_share_full_request_tier_and_legacy_optional_keys() {
    let monday = Date::from_calendar_date(2026, Month::October, 26).unwrap();
    let at = monday.midnight();
    let model = json!({
        "cost":{"input":0.01,"output":0.01},
        "cost200K":{"threshold":2,"input":0.02,"output":0.03,"cacheRead":0.04,"cacheWrite5m":0.05,"cacheWrite1h":0.06},
        "costPeak":{"input":0.07,"output":0.08,"cacheRead":0.09,"cacheWrite5m":0.10,"cacheWrite1h":0.11}
    });
    let usage = UsageInfo {
        input_tokens: 1,
        output_tokens: 2,
        cache_read_tokens: Some(1),
        cache_write5m_tokens: Some(1),
        cache_write1h_tokens: Some(1),
        reasoning_tokens: Some(999),
    };
    let metrics = calculate_cost_components(&model, &usage, at).unwrap();
    assert_eq!(metrics["cost.input.microcents"], 2_000_000);
    assert_eq!(metrics["cost.output.microcents"], 6_000_000);
    assert_eq!(metrics["cost.cache_read.microcents"], 4_000_000);
    assert_eq!(metrics["cost.cache_write.microcents"], 5_000_000);
    assert_eq!(metrics["cost.cache_write_1h"], 6);
    assert_eq!(metrics["cost.total.microcents"], 23_000_000);
    assert_eq!(
        metrics["cost.total.microcents"].as_i64(),
        Some(calculate_cost(&model, &usage, at).unwrap())
    );
    assert!(metrics.get("rawCents").is_none());
    assert!(metrics.get("cost.cache_write_1h.microcents").is_none());
    let peak =
        calculate_cost_components(&model, &usage, monday.with_hms(1, 0, 0).unwrap()).unwrap();
    assert_eq!(peak["cost.input.microcents"], 7_000_000);
    assert_eq!(peak["cost.output.microcents"], 16_000_000);
    let optional = calculate_cost_components(
        &json!({"cost":{"input":0.000000005,"output":0,"cacheRead":0}}),
        &UsageInfo {
            input_tokens: -3,
            cache_read_tokens: Some(4),
            cache_write5m_tokens: Some(0),
            ..Default::default()
        },
        at,
    )
    .unwrap();
    assert_eq!(optional["cost.input.microcents"], -2);
    assert_eq!(optional["cost.input"], 0);
    assert!(optional.get("cost.cache_read").is_none());
    assert!(optional.get("cost.cache_write.microcents").is_none());
}

#[test]
fn pricing_signed_rounding_tiers_and_china_peak_boundaries() {
    let monday = Date::from_calendar_date(2026, Month::October, 26).unwrap();
    let model = json!({"cost":{"input":0.000000005,"output":0},"cost200K":{"threshold":2,"input":0.00000002,"output":0},"costPeak":{"input":0.00000003,"output":0}});
    for (hour, minute, second, tokens, expected) in [
        (0, 59, 59, 1, 1),
        (0, 59, 59, 2, 1),
        (0, 59, 59, 3, 6),
        (1, 0, 0, 3, 9),
        (4, 0, 0, 3, 6),
        (6, 0, 0, 3, 9),
        (10, 0, 0, 3, 6),
        (0, 59, 59, -1, 0),
        (0, 59, 59, -3, -2),
    ] {
        let usage = UsageInfo {
            input_tokens: tokens,
            ..Default::default()
        };
        assert_eq!(
            calculate_cost(
                &model,
                &usage,
                monday.with_hms(hour, minute, second).unwrap()
            )
            .unwrap(),
            expected
        );
    }
    let cached = UsageInfo {
        input_tokens: 1,
        cache_read_tokens: Some(2),
        reasoning_tokens: Some(500),
        ..Default::default()
    };
    assert_eq!(
        calculate_cost(&model, &cached, monday.with_hms(0, 0, 0).unwrap()).unwrap(),
        2
    );
    let saturday = Date::from_calendar_date(2026, Month::October, 24)
        .unwrap()
        .with_hms(1, 0, 0)
        .unwrap();
    assert_eq!(
        calculate_cost(
            &model,
            &UsageInfo {
                input_tokens: 1,
                ..Default::default()
            },
            saturday
        )
        .unwrap(),
        1
    );
}

#[test]
fn pricing_all_cache_fields_and_catalog_default_threshold() {
    let at = Date::from_calendar_date(2026, Month::October, 24)
        .unwrap()
        .midnight();
    let model = json!({"cost":{"input":0.000001,"output":0.000002,"cacheRead":0.000003,"cacheWrite5m":0.000004,"cacheWrite1h":0.000005}});
    let usage = UsageInfo {
        input_tokens: 1,
        output_tokens: 1,
        reasoning_tokens: Some(900),
        cache_read_tokens: Some(1),
        cache_write5m_tokens: Some(1),
        cache_write1h_tokens: Some(1),
    };
    assert_eq!(calculate_cost(&model, &usage, at).unwrap(), 1500);
    let model =
        json!({"cost":{"input":0.000001,"output":0},"cost200K":{"input":0.000002,"output":0}});
    assert_eq!(
        calculate_cost(
            &model,
            &UsageInfo {
                input_tokens: 200000,
                ..Default::default()
            },
            at
        )
        .unwrap(),
        20000000
    );
    assert_eq!(
        calculate_cost(
            &model,
            &UsageInfo {
                input_tokens: 200000,
                cache_write1h_tokens: Some(1),
                ..Default::default()
            },
            at
        )
        .unwrap(),
        40000000
    );
}
