//! Purpose: pure tests for legacy retry/backoff, overflow and usage/cost policies
//! (packages/opencode/src/session/{retry,overflow}.ts, session.ts getUsage).
//! Scope: test-only, no I/O.
//! Uses:
//! rust/crates/
//! `-- session-runtime/src/legacy/policy.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: expected values are computed by hand from the baseline formulas.
//! Contract tests: session-runtime/tests/legacy_policy.rs

use opencode_session_runtime::legacy::policy::{
    GO_UPSELL_MESSAGE, Limits, delay, get_usage, is_overflow, max_output_tokens, policy_step,
    retryable, usable,
};
use serde_json::json;

fn api(data: serde_json::Value) -> serde_json::Value {
    json!({ "name": "APIError", "data": data })
}

#[test]
fn delay_uses_headers_then_exponential_with_caps() {
    let headers = |h: serde_json::Value| {
        api(json!({ "message": "x", "isRetryable": true, "responseHeaders": h }))
    };
    assert_eq!(
        delay(
            1,
            Some(&headers(json!({ "retry-after-ms": "1500.5" }))),
            0.0,
            0.0
        ),
        1500.5
    );
    assert_eq!(
        delay(1, Some(&headers(json!({ "retry-after": "2.2" }))), 0.0, 0.0),
        2200.0
    );
    let date = headers(json!({ "retry-after": "Thu, 01 Jan 1970 00:00:10 GMT" }));
    assert_eq!(delay(1, Some(&date), 0.0, 4_000.0), 6_000.0);
    // Headers present but unusable: uncapped-by-30s exponential.
    assert_eq!(delay(6, Some(&headers(json!({}))), 0.0, 0.0), 64_000.0);
    // No headers: exponential with jitter, capped at 30s.
    assert_eq!(delay(1, None, 0.0, 0.0), 2000.0);
    assert_eq!(delay(2, None, 1.0, 0.0), 5000.0);
    assert_eq!(delay(6, None, 0.0, 0.0), 30_000.0);
}

#[test]
fn retryable_classification() {
    assert!(
        retryable(
            &json!({ "name": "ContextOverflowError", "data": { "message": "rate limit" } }),
            "p"
        )
        .is_none()
    );
    assert!(
        retryable(
            &api(json!({ "message": "bad request", "isRetryable": false, "statusCode": 400 })),
            "p"
        )
        .is_none()
    );
    assert_eq!(
        retryable(
            &api(json!({ "message": "boom", "isRetryable": false, "statusCode": 503 })),
            "p"
        )
        .unwrap()
        .message,
        "boom"
    );
    assert_eq!(
        retryable(
            &api(json!({ "message": "Overloaded", "isRetryable": true })),
            "p"
        )
        .unwrap()
        .message,
        "Provider is overloaded"
    );
    let free = retryable(
        &api(json!({ "message": "x", "isRetryable": true, "responseBody": "FreeUsageLimitError" })),
        "op",
    )
    .unwrap();
    assert_eq!(free.message, GO_UPSELL_MESSAGE);
    assert_eq!(free.action.unwrap()["provider"], "op");
    let go = retryable(
        &api(json!({
            "message": "x", "isRetryable": true,
            "responseBody": "{\"type\":\"GoUsageLimitError\",\"metadata\":{\"workspace\":\"w1\",\"limitName\":\"Daily\"}}",
            "responseHeaders": { "retry-after": "3700" },
        })),
        "op",
    )
    .unwrap();
    assert_eq!(
        go.message,
        "Daily usage limit reached. It will reset in 1 hour 2 minutes. To continue using this model now, enable usage from your available balance - https://opencode.ai/workspace/w1/go"
    );
    assert_eq!(
        retryable(
            &json!({ "name": "UnknownError", "data": { "message": "TOO_MANY_REQUESTS" } }),
            "p"
        )
        .unwrap()
        .message,
        "Too Many Requests"
    );
    assert_eq!(
        retryable(
            &json!({ "name": "UnknownError", "data": { "message": "Resource exhausted" } }),
            "p"
        )
        .unwrap()
        .message,
        "Provider is overloaded"
    );
    assert_eq!(
        retryable(
            &json!({ "name": "UnknownError", "data": { "message": "socket hang up" } }),
            "p"
        )
        .unwrap()
        .message,
        "socket hang up"
    );
    assert!(
        retryable(
            &json!({ "name": "UnknownError", "data": { "message": "nope" } }),
            "p"
        )
        .is_none()
    );
}

#[test]
fn policy_step_stops_after_max_retries() {
    let error = api(json!({ "message": "rate limit", "isRetryable": true }));
    let (wait, status) = policy_step(&error, "p", 1, 0.0, 100.0).unwrap();
    assert_eq!(wait, 2000.0);
    assert_eq!(
        status,
        json!({ "type": "retry", "attempt": 1, "message": "rate limit", "next": 2100.0 })
    );
    assert!(policy_step(&error, "p", 6, 0.0, 0.0).is_none());
}

#[test]
fn overflow_uses_input_limit_or_context_minus_output() {
    let limits = Limits {
        context: 100_000.0,
        input: None,
        output: 8_000.0,
    };
    assert_eq!(max_output_tokens(limits, None), 8_000.0);
    assert_eq!(
        max_output_tokens(
            Limits {
                output: 0.0,
                ..limits
            },
            None
        ),
        32_000.0
    );
    assert_eq!(usable(None, limits, None), 92_000.0);
    assert_eq!(
        usable(
            None,
            Limits {
                input: Some(50_000.0),
                ..limits
            },
            None
        ),
        42_000.0
    );
    assert_eq!(
        usable(
            Some(1_000.0),
            Limits {
                input: Some(50_000.0),
                ..limits
            },
            None
        ),
        49_000.0
    );
    let tokens = json!({ "input": 90_000, "output": 1_000, "reasoning": 0, "cache": { "read": 1_000, "write": 0 } });
    assert!(is_overflow(None, None, &tokens, limits, None));
    assert!(!is_overflow(Some(false), None, &tokens, limits, None));
    assert!(!is_overflow(
        None,
        None,
        &json!({ "total": 10, "input": 99_999 }),
        limits,
        None
    ));
    assert!(!is_overflow(
        None,
        None,
        &tokens,
        Limits {
            context: 0.0,
            ..limits
        },
        None
    ));
}

#[test]
fn usage_splits_cache_and_reasoning_and_prices_tiers() {
    let cost = json!({ "input": 1.0, "output": 2.0, "cache": { "read": 0.5, "write": 4.0 },
        "tiers": [{ "tier": { "type": "context", "size": 1000 }, "input": 10.0, "output": 20.0, "cache": { "read": 5.0, "write": 40.0 } }] });
    let usage = json!({ "inputTokens": 1_000_000, "outputTokens": 500_000, "reasoningTokens": 100_000, "cacheReadInputTokens": 200_000, "totalTokens": 1_500_000 });
    let metadata = json!({ "anthropic": { "cacheCreationInputTokens": 100_000 } });
    let result = get_usage(Some(&cost), &usage, Some(&metadata));
    assert_eq!(
        result["tokens"],
        json!({ "total": 1_500_000, "input": 700_000.0, "output": 400_000.0, "reasoning": 100_000.0, "cache": { "write": 100_000.0, "read": 200_000.0 } })
    );
    // Tier rates: 0.7*10 + 0.4*20 + 0.2*5 + 0.1*40 + 0.1*20 = 7 + 8 + 1 + 4 + 2.
    assert_eq!(result["cost"], json!(22.0));
    let copilot = get_usage(
        Some(&cost),
        &usage,
        Some(&json!({ "copilot": { "totalNanoAiu": 250_000_000_000.0 } })),
    );
    assert_eq!(copilot["cost"], json!(2.5));
    let none = get_usage(None, &json!({ "inputTokens": -5 }), None);
    assert_eq!(none["cost"], json!(0.0));
    assert!(none["tokens"].get("total").is_none());
}
