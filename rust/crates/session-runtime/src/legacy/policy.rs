//! Purpose: legacy provider-turn policies: `SessionRetry` backoff and retryability
//! (packages/opencode/src/session/retry.ts:1-209), `SessionOverflow.usable/isOverflow`
//! (overflow.ts:1-34), `ProviderTransform.maxOutputTokens` and `Session.getUsage` token/cost
//! accounting (session.ts:338-405).
//! Scope: pure functions; callers own clocks, sleeping and persistence.
//! Uses:
//! `-- none
//! External:
//! |-- f64::INFINITY
//! |-- f64::NAN
//! |-- f64::from
//! |-- jiff::fmt::rfc2822::parse
//! |-- regex::Regex
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! `-- u8::from
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/compaction.rs
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/legacy_policy.rs
//! Invariants: delays are capped at 2^31-1 ms, header-less exponential delays at 30 s; at most 5
//! retries; context overflow is never retried; 5xx is always retryable. Costs are per-million rates
//! applied to non-overlapping token buckets, reasoning billed at the output rate, clamped at zero.
//! Contract tests: session-runtime/tests/legacy_policy.rs

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

pub const RETRY_INITIAL_DELAY: f64 = 2000.0;
pub const RETRY_BACKOFF_FACTOR: f64 = 2.0;
pub const RETRY_JITTER_FACTOR: f64 = 0.25;
pub const RETRY_MAX_DELAY_NO_HEADERS: f64 = 30_000.0;
pub const RETRY_MAX_DELAY: f64 = 2_147_483_647.0;
pub const RETRY_MAX_RETRIES: u32 = 5;
pub const GO_UPSELL_MESSAGE: &str = "Free usage exceeded, subscribe to Go";
pub const GO_UPSELL_URL: &str = "https://opencode.ai/go";
pub const COMPACTION_BUFFER: f64 = 20_000.0;
pub const OUTPUT_TOKEN_MAX: f64 = 32_000.0;

static RETRYABLE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)429|500|502|503|504|524",
        r"(?i)rate increased too quickly|rate limit|rate-limit|rate_limit|too many requests",
        r"(?i)overloaded|service unavailable|service_unavailable|service-unavailable|internal error|internal_error|internal server error|server error|server_error|server-error|provider returned error|provider_returned_error|provider-returned-error",
        r"(?i)terminated|fetch failed|failed to fetch|network[-_\s]error|upstream connect|connection error|connection refused|connection lost|socket connection was closed|socket hang up|reset before headers|getaddrinfo|enotfound|eai_again|econnrefused|econnreset|etimedout",
        r"(?i)^timeout$|\b(?:request|response|connection|network|stream|read) (?:timeout|timed out|time out)\b",
        r"(?i)try your request again|retry your request|resource exhausted|resource_exhausted",
        r"(?i)\btry again (?:later|in\b)|\b(?:currently|temporarily) at capacity\b",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("static retry pattern"))
    .collect()
});

fn matches_retryable(value: Option<&str>) -> bool {
    value.is_some_and(|value| RETRYABLE.iter().any(|pattern| pattern.is_match(value)))
}

fn cap(ms: f64) -> f64 {
    ms.min(RETRY_MAX_DELAY)
}

fn exponential(attempt: u32, random: f64) -> f64 {
    let base = RETRY_INITIAL_DELAY * RETRY_BACKOFF_FACTOR.powi(attempt as i32 - 1);
    (base + base * RETRY_JITTER_FACTOR * random).ceil()
}

/// JS `Number.parseFloat`: the longest numeric prefix, else `None` (NaN).
fn parse_float(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    let mut end = 0;
    let bytes = trimmed.as_bytes();
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end += 1;
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end == digits_start || trimmed[digits_start..end] == *"." {
        return trimmed.starts_with("Infinity").then_some(f64::INFINITY);
    }
    if end < bytes.len() && matches!(bytes[end], b'e' | b'E') {
        let mut exp = end + 1;
        if matches!(bytes.get(exp), Some(b'+' | b'-')) {
            exp += 1;
        }
        let start = exp;
        while exp < bytes.len() && bytes[exp].is_ascii_digit() {
            exp += 1;
        }
        if exp > start {
            end = exp;
        }
    }
    trimmed[..end].parse().ok()
}

/// `SessionRetry.delay(attempt, error?, random)`. `error` is an `APIError` object
/// (`{ name, data: { responseHeaders? } }`); `now_ms` resolves HTTP-date `retry-after` values.
pub fn delay(attempt: u32, error: Option<&Value>, random: f64, now_ms: f64) -> f64 {
    if let Some(headers) = error
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("responseHeaders"))
        .and_then(Value::as_object)
    {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
        };
        if let Some(ms) = header("retry-after-ms").and_then(parse_float) {
            return cap(ms);
        }
        if let Some(value) = header("retry-after") {
            if let Some(seconds) = parse_float(value) {
                return cap((seconds * 1000.0).ceil());
            }
            if let Some(date) = parse_http_date(value) {
                let parsed = date - now_ms;
                if parsed > 0.0 {
                    return cap(parsed.ceil());
                }
            }
        }
        return cap(exponential(attempt, random));
    }
    cap(exponential(attempt, random).min(RETRY_MAX_DELAY_NO_HEADERS))
}

/// `Date.parse` for RFC 7231 IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`), epoch ms.
fn parse_http_date(value: &str) -> Option<f64> {
    let parsed = jiff::fmt::rfc2822::parse(value).ok()?;
    Some(parsed.timestamp().as_millisecond() as f64)
}

/// `Retryable.action`.
#[derive(Clone, Debug, PartialEq)]
pub struct Retryable {
    pub message: String,
    pub action: Option<Value>,
}

/// `SessionRetry.retryable(error, provider)` over a `NamedError.toObject()` value.
pub fn retryable(error: &Value, provider: &str) -> Option<Retryable> {
    let name = error.get("name").and_then(Value::as_str);
    let data = error.get("data");
    if name == Some("ContextOverflowError") {
        return None;
    }
    let field = |key: &str| data.and_then(|data| data.get(key));
    let text = |key: &str| field(key).and_then(Value::as_str);
    if name == Some("APIError") {
        let status = field("statusCode").and_then(Value::as_f64);
        let flagged = field("isRetryable").and_then(Value::as_bool) == Some(true);
        if !flagged
            && !status.is_some_and(|status| status >= 500.0)
            && !matches_retryable(text("message"))
            && !matches_retryable(text("responseBody"))
        {
            return None;
        }
        let body = text("responseBody");
        if body.is_some_and(|body| body.contains("FreeUsageLimitError")) {
            return Some(Retryable {
                message: GO_UPSELL_MESSAGE.into(),
                action: Some(json!({
                    "reason": "free_tier_limit",
                    "provider": provider,
                    "title": "Free limit reached",
                    "message": "Subscribe to OpenCode Go for reliable access to the best open-source models for $10/month.",
                    "label": "subscribe",
                    "link": GO_UPSELL_URL,
                })),
            });
        }
        if let Some(body) = body.filter(|body| body.contains("GoUsageLimitError")) {
            return Some(go_limit(body, field("responseHeaders"), provider));
        }
        let message = text("message").unwrap_or_default();
        let message = if message.contains("Overloaded") {
            "Provider is overloaded".to_owned()
        } else {
            message.to_owned()
        };
        return Some(Retryable {
            message,
            action: None,
        });
    }
    let message = data
        .filter(|d| d.is_object())
        .and_then(|d| d.get("message"))?
        .as_str()?;
    let lower = message.to_lowercase();
    if lower.contains("too_many_requests") {
        return Some(Retryable {
            message: "Too Many Requests".into(),
            action: None,
        });
    }
    if lower.contains("exhausted") || lower.contains("unavailable") {
        return Some(Retryable {
            message: "Provider is overloaded".into(),
            action: None,
        });
    }
    matches_retryable(Some(message)).then(|| Retryable {
        message: message.into(),
        action: None,
    })
}

fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn go_limit(body: &str, headers: Option<&Value>, provider: &str) -> Retryable {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let metadata = parsed.as_ref().and_then(|body| body.get("metadata"));
    let workspace = js_string(metadata.and_then(|m| m.get("workspace")));
    let limit_name = js_string(metadata.and_then(|m| m.get("limitName")));
    let retry_after = parse_float(&js_string(headers.and_then(|h| h.get("retry-after"))));
    let reset_in = match retry_after {
        None => String::new(),
        Some(retry_after) => {
            let seconds = retry_after.ceil().max(0.0) as u64;
            let days = seconds / 86_400;
            let hours = (seconds % 86_400) / 3_600;
            let minutes = (seconds % 3_600).div_ceil(60);
            let unit = |value: u64, name: &str| {
                format!("{value} {name}{}", if value == 1 { "" } else { "s" })
            };
            if days > 0 {
                if hours > 0 {
                    format!("{} {}", unit(days, "day"), unit(hours, "hour"))
                } else {
                    unit(days, "day")
                }
            } else if hours > 0 {
                if minutes > 0 {
                    format!("{} {}", unit(hours, "hour"), unit(minutes, "minute"))
                } else {
                    unit(hours, "hour")
                }
            } else if minutes > 0 {
                unit(minutes, "minute")
            } else {
                "less than a minute".into()
            }
        }
    };
    let prefix = if limit_name.is_empty() {
        "Usage limit".to_owned()
    } else {
        format!("{limit_name} usage limit")
    };
    let message = format!(
        "{prefix} reached. It will reset in {reset_in}. To continue using this model now, enable usage from your available balance"
    );
    let link = format!("https://opencode.ai/workspace/{workspace}/go");
    Retryable {
        message: format!("{message} - {link}"),
        action: Some(json!({
            "reason": "account_rate_limit",
            "provider": provider,
            "title": "Go limit reached",
            "message": message,
            "label": "open settings",
            "link": link,
        })),
    }
}

/// One `SessionRetry.policy` step: `None` stops retrying, else the delay and status to publish.
pub fn policy_step(
    error: &Value,
    provider: &str,
    attempt: u32,
    random: f64,
    now_ms: f64,
) -> Option<(f64, Value)> {
    let retry = retryable(error, provider)?;
    if attempt > RETRY_MAX_RETRIES {
        return None;
    }
    let is_api = error.get("name").and_then(Value::as_str) == Some("APIError");
    let wait = delay(attempt, is_api.then_some(error), random, now_ms);
    let mut status = json!({
        "type": "retry",
        "attempt": attempt,
        "message": retry.message,
        "next": now_ms + wait,
    });
    if let Some(action) = retry.action {
        status["action"] = action;
    }
    Some((wait, status))
}

/// Model limits read by overflow checks (`Provider.Model.limit`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub context: f64,
    pub input: Option<f64>,
    pub output: f64,
}

/// `ProviderTransform.maxOutputTokens`: `min(limit.output, max) || max`.
pub fn max_output_tokens(limits: Limits, output_token_max: Option<f64>) -> f64 {
    let max = output_token_max.unwrap_or(OUTPUT_TOKEN_MAX);
    let value = limits.output.min(max);
    if value == 0.0 || value.is_nan() {
        max
    } else {
        value
    }
}

/// `SessionOverflow.usable`.
pub fn usable(reserved: Option<f64>, limits: Limits, output_token_max: Option<f64>) -> f64 {
    if limits.context == 0.0 {
        return 0.0;
    }
    let max_output = max_output_tokens(limits, output_token_max);
    let reserved = reserved.unwrap_or_else(|| COMPACTION_BUFFER.min(max_output));
    match limits.input {
        Some(input) if input != 0.0 => (input - reserved).max(0.0),
        _ => (limits.context - max_output).max(0.0),
    }
}

/// `SessionOverflow.isOverflow`. `tokens` is the assistant `tokens` object.
pub fn is_overflow(
    auto: Option<bool>,
    reserved: Option<f64>,
    tokens: &Value,
    limits: Limits,
    output_token_max: Option<f64>,
) -> bool {
    if auto == Some(false) || limits.context == 0.0 {
        return false;
    }
    let n = |value: &Value| value.as_f64().unwrap_or(0.0);
    let total = n(&tokens["total"]);
    let count = if total != 0.0 {
        total
    } else {
        n(&tokens["input"])
            + n(&tokens["output"])
            + n(&tokens["cache"]["read"])
            + n(&tokens["cache"]["write"])
    };
    count >= usable(reserved, limits, output_token_max)
}

/// `Session.getUsage`: AI SDK usage plus provider metadata to `{ cost, tokens }`.
/// `model_cost` is `Provider.Model.cost` (`{ input, output, cache: { read, write }, tiers?,
/// experimentalOver200K? }`).
pub fn get_usage(model_cost: Option<&Value>, usage: &Value, metadata: Option<&Value>) -> Value {
    let finite = |value: f64| if value.is_finite() { value } else { 0.0 };
    let safe = |value: f64| finite(value).max(0.0);
    let num = |value: Option<&Value>| value.and_then(Value::as_f64);
    let input_tokens = safe(num(usage.get("inputTokens")).unwrap_or(0.0));
    let output_tokens = safe(num(usage.get("outputTokens")).unwrap_or(0.0));
    let reasoning_tokens = safe(num(usage.get("reasoningTokens")).unwrap_or(0.0));
    let cache_read = safe(num(usage.get("cacheReadInputTokens")).unwrap_or(0.0));
    let meta = |path: &[&str]| {
        path.iter()
            .try_fold(metadata?, |value, key| value.get(*key))
            .filter(|value| !value.is_null())
    };
    let cache_write_raw = usage
        .get("cacheWriteInputTokens")
        .filter(|value| !value.is_null())
        .or_else(|| meta(&["anthropic", "cacheCreationInputTokens"]))
        .or_else(|| meta(&["vertex", "cacheCreationInputTokens"]))
        .or_else(|| meta(&["bedrock", "usage", "cacheWriteInputTokens"]))
        .or_else(|| meta(&["venice", "usage", "cacheCreationInputTokens"]));
    let cache_write = safe(match cache_write_raw {
        None => 0.0,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.trim().parse().unwrap_or(f64::NAN),
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(_) => f64::NAN,
    });
    let adjusted_input = safe(input_tokens - cache_read - cache_write);
    let mut tokens = json!({
        "input": adjusted_input,
        "output": safe(output_tokens - reasoning_tokens),
        "reasoning": reasoning_tokens,
        "cache": { "write": cache_write, "read": cache_read },
    });
    if let Some(total) = usage.get("totalTokens").filter(|value| !value.is_null()) {
        let mut ordered = serde_json::Map::new();
        ordered.insert("total".into(), total.clone());
        ordered.extend(tokens.as_object().cloned().unwrap_or_default());
        tokens = Value::Object(ordered);
    }
    let context = input_tokens;
    let tier = model_cost
        .and_then(|cost| cost.get("tiers"))
        .and_then(Value::as_array)
        .and_then(|tiers| {
            tiers
                .iter()
                .filter(|item| {
                    item["tier"]["type"] == "context"
                        && item["tier"]["size"]
                            .as_f64()
                            .is_some_and(|size| context > size)
                })
                .max_by(|a, b| {
                    let size = |v: &Value| v["tier"]["size"].as_f64().unwrap_or(0.0);
                    size(a).total_cmp(&size(b))
                })
        });
    let over = model_cost
        .and_then(|cost| cost.get("experimentalOver200K"))
        .filter(|value| !value.is_null() && context > 200_000.0);
    let rates = tier.or(over).or(model_cost);
    let rate = |path: &[&str]| {
        finite(
            path.iter()
                .try_fold(rates.unwrap_or(&Value::Null), |value, key| value.get(*key))
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        )
    };
    let copilot = meta(&["copilot", "totalNanoAiu"]).and_then(Value::as_f64);
    let cost = match copilot {
        Some(nano) if nano.is_finite() && nano >= 0.0 => nano / 100_000_000_000.0,
        _ => safe(
            (adjusted_input * rate(&["input"])
                + tokens["output"].as_f64().unwrap_or(0.0) * rate(&["output"])
                + cache_read * rate(&["cache", "read"])
                + cache_write * rate(&["cache", "write"])
                + reasoning_tokens * rate(&["output"]))
                / 1_000_000.0,
        ),
    };
    json!({ "cost": cost, "tokens": tokens })
}
