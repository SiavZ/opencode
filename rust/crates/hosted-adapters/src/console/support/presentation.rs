//! Purpose: render support usage diagnostics with baseline currency and UTC window rules.
//! Scope: pure support presentation helpers.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::{Value, json}
//! `-- time::{Date, Duration, Month, OffsetDateTime}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/support/lookup.rs
//! Invariants: weekly windows start Monday UTC; monthly anchors clamp to month length; no raw key output.
//! Contract tests: hosted-adapters/tests/console_support.rs

use serde_json::{Value, json};
use time::{Date, Duration, Month, OffsetDateTime};

pub(super) fn dollars(value: i64) -> String {
    format!("${:.2}", value as f64 / 100_000_000.0)
}
pub(super) fn date(value: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(value)
        .ok()
        .map(|v| format!("{:04}-{:02}-{:02}", v.year(), u8::from(v.month()), v.day()))
}
fn stamp(value: &Value, fallback: i64) -> i64 {
    value.as_f64().map(|v| v as i64).unwrap_or(fallback)
}
fn week(now: OffsetDateTime) -> (i64, i64) {
    let start = (now.date() - Duration::days(now.weekday().number_days_from_monday().into()))
        .midnight()
        .assume_utc()
        .unix_timestamp();
    (start, start + 7 * 86400)
}
fn month_anchor(year: i32, month: u8, subscribed: OffsetDateTime) -> OffsetDateTime {
    let month = Month::try_from(month).unwrap();
    let next = if month == Month::December {
        Date::from_calendar_date(year + 1, Month::January, 1).unwrap()
    } else {
        Date::from_calendar_date(year, Month::try_from(u8::from(month) + 1).unwrap(), 1).unwrap()
    };
    let day = subscribed.day().min((next - Duration::days(1)).day());
    Date::from_calendar_date(year, month, day)
        .unwrap()
        .with_time(subscribed.time())
        .assume_utc()
}
fn month(now: OffsetDateTime, subscribed: OffsetDateTime) -> (i64, i64) {
    let index = now.year() * 12 + i32::from(u8::from(now.month())) - 1;
    let start = month_anchor(now.year(), u8::from(now.month()), subscribed);
    let index = index - i32::from(start > now);
    let next = index + 1;
    (
        month_anchor(
            index.div_euclid(12),
            (index.rem_euclid(12) + 1) as u8,
            subscribed,
        )
        .unix_timestamp(),
        month_anchor(
            next.div_euclid(12),
            (next.rem_euclid(12) + 1) as u8,
            subscribed,
        )
        .unix_timestamp(),
    )
}
fn reset(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".into();
    }
    if seconds >= 86400 {
        return format!("{}d", seconds / 86400);
    }
    if seconds >= 3600 {
        return if seconds % 3600 / 60 > 0 {
            format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
        } else {
            format!("{}h", seconds / 3600)
        };
    }
    format!("{}m", (seconds + 59) / 60)
}
fn usage(value: i64, limit: i64, reset_in: i64) -> String {
    let limited = value >= limit;
    let percent = if limited {
        100
    } else if limit != 0 {
        ((value as f64 / limit as f64) * 100.0).min(100.0).floor() as i64
    } else {
        0
    };
    format!(
        "{percent}% (resets in {}){}",
        reset(reset_in),
        if limited { " [limited]" } else { "" }
    )
}
pub(super) fn go(row: Value, limits: &Value, now: OffsetDateTime) -> Value {
    let limits = &limits["lite"];
    let now_stamp = now.unix_timestamp();
    let rolling_stamp = stamp(&row["timeRollingUpdated"], now_stamp);
    let window = limits["rollingWindow"].as_i64().unwrap_or(5) * 3600;
    let weekly = week(now);
    let subscribed =
        OffsetDateTime::from_unix_timestamp(stamp(&row["timeCreated"], now_stamp)).unwrap_or(now);
    let monthly = month(now, subscribed);
    let counter = |key: &str, stamp_key: &str, start: i64| {
        if stamp(&row[stamp_key], now_stamp) < start {
            0
        } else {
            row[key].as_i64().unwrap_or_default()
        }
    };
    json!({
        "email":row["authEmail"].as_str().or(row["userEmail"].as_str()).or(row["userID"].as_str()),
        "subscribed":date(subscribed.unix_timestamp()),
        "useBalance":if row["useBalance"]["useBalance"].as_bool().unwrap_or(false) {"yes"} else {"no"},
        "rolling":usage(counter("rollingUsage","timeRollingUpdated",now_stamp-window),limits["rollingLimit"].as_i64().unwrap_or_default()*100_000_000,if rolling_stamp<now_stamp-window {window} else {rolling_stamp+window-now_stamp}),
        "weekly":usage(counter("weeklyUsage","timeWeeklyUpdated",weekly.0),limits["weeklyLimit"].as_i64().unwrap_or_default()*100_000_000,weekly.1-now_stamp),
        "monthly":usage(counter("monthlyUsage","timeMonthlyUpdated",monthly.0),limits["monthlyLimit"].as_i64().unwrap_or_default()*100_000_000,monthly.1-now_stamp)
    })
}
pub(super) fn user(row: Value, limits: &Value, now: OffsetDateTime) -> Value {
    let email = row["authEmail"]
        .as_str()
        .or(row["inviteEmail"].as_str())
        .unwrap_or("null");
    let deleted = row["deleted"]
        .as_bool()
        .unwrap_or_else(|| row["deleted"].as_i64() == Some(1));
    let monthly = format!(
        "{} / {}",
        dollars(row["monthlyUsage"].as_i64().unwrap_or_default()),
        row["monthlyLimit"]
            .as_i64()
            .map(|v| format!("${:.2}", v as f64))
            .unwrap_or_else(|| "no limit".into())
    );
    let mut result = json!({"email":format!("{}{email}",if deleted {"[deleted] "} else {""}),"role":row["role"],"timeSeen":row["timeSeen"],"monthly":monthly,"subscribed":row["subscribed"],"subWeekly":null,"subRolling":null,"rateLimited":null,"retryIn":null});
    if row["subscribed"].is_null() || row["subscription"].is_null() {
        return result;
    }
    let plan = row["subscription"]["plan"].as_str().unwrap_or_default();
    let limits = &limits["black"][plan];
    let weekly = week(now);
    let window = limits["rollingWindow"].as_i64().unwrap_or(5) * 3600;
    let fixed = if stamp(&row["timeFixedUpdated"], 0) >= weekly.0 {
        row["fixedUsage"].as_i64().unwrap_or_default()
    } else {
        0
    };
    let rolling_stamp = stamp(&row["timeRollingUpdated"], 0);
    let rolling = if rolling_stamp >= now.unix_timestamp() - window {
        row["rollingUsage"].as_i64().unwrap_or_default()
    } else {
        0
    };
    let fixed_limit = limits["fixedLimit"].as_i64().filter(|v| *v != 0);
    let rolling_limit = limits["rollingLimit"].as_i64().filter(|v| *v != 0);
    let weekly_limited = fixed_limit.is_some_and(|v| fixed >= v * 100_000_000);
    let rolling_limited = rolling_limit.is_some_and(|v| rolling >= v * 100_000_000);
    result["subWeekly"] = json!(fixed_limit.map(|v| format!("{} / ${v}", dollars(fixed))));
    result["subRolling"] = json!(rolling_limit.map(|v| format!("{} / ${v}", dollars(rolling))));
    result["rateLimited"] = json!(if weekly_limited || rolling_limited {
        "yes"
    } else {
        "no"
    });
    let seconds = if weekly_limited {
        Some(weekly.1 - now.unix_timestamp())
    } else if rolling_limited {
        Some(rolling_stamp + window - now.unix_timestamp())
    } else {
        None
    };
    result["retryIn"] = json!(seconds.map(|s| if s >= 86400 {
        format!("{} day{}", s / 86400, if s / 86400 > 1 { "s" } else { "" })
    } else if s >= 3600 {
        format!("{}hr {}min", s / 3600, (s % 3600 + 59) / 60)
    } else {
        format!("{}min", (s + 59) / 60)
    }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_monthly_reset_changes_after_the_creation_second() {
        let subscribed = Date::from_calendar_date(2026, Month::October, 27)
            .unwrap()
            .with_hms(5, 46, 53)
            .unwrap()
            .assume_utc();
        let row = json!({"userID":"usr_fixture","timeCreated":subscribed.unix_timestamp(),"monthlyUsage":0});
        let limits = json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":50,"monthlyLimit":100}});
        assert_eq!(
            go(row.clone(), &limits, subscribed)["monthly"],
            "0% (resets in 31d)"
        );
        assert_eq!(
            go(row, &limits, subscribed + Duration::seconds(1))["monthly"],
            "0% (resets in 30d)"
        );
    }

    #[test]
    fn support_windows_clamp_month_end_and_begin_weeks_on_monday() {
        let subscribed = Date::from_calendar_date(2024, Month::January, 31)
            .unwrap()
            .with_hms(10, 30, 0)
            .unwrap()
            .assume_utc();
        let now = Date::from_calendar_date(2024, Month::March, 15)
            .unwrap()
            .midnight()
            .assume_utc();
        let (start, end) = month(now, subscribed);
        assert_eq!(date(start).as_deref(), Some("2024-02-29"));
        assert_eq!(date(end).as_deref(), Some("2024-03-31"));
        assert_eq!(date(week(now).0).as_deref(), Some("2024-03-11"));
        let limits = json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":50,"monthlyLimit":100}});
        let row = json!({"userID":"usr_fixture","timeCreated":subscribed.unix_timestamp(),"rollingUsage":9999999999_i64,"timeRollingUpdated":now.unix_timestamp()-18001,"weeklyUsage":5000000000_i64,"timeWeeklyUpdated":now.unix_timestamp(),"monthlyUsage":0});
        let go = go(row, &limits, now);
        assert_eq!(go["rolling"], "0% (resets in 5h)");
        assert!(go["weekly"].as_str().unwrap().ends_with(" [limited]"));
        let user = user(
            json!({"monthlyLimit":15,"monthlyUsage":123000000}),
            &Value::Null,
            now,
        );
        assert_eq!(user["monthly"], "$1.23 / $15.00");
    }
}
