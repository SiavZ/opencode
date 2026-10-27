//! Purpose: validate quota reset requests and aggregate before-reset usage counters.
//! Scope: pure quota rules; persistence is handled by the billing store.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/common.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: only active rows count; reset results sum null counters as zero and do not modify window stamps.
//! Contract tests: hosted-domain/src/console/quotas.rs

use serde::{Deserialize, Serialize};
use time::{Date, Duration, Month, PrimitiveDateTime};

use super::common::{ConsoleError, MicroCents};

pub fn week(at: PrimitiveDateTime) -> (PrimitiveDateTime, PrimitiveDateTime) {
    let start =
        (at.date() - Duration::days(i64::from(at.weekday().number_days_from_monday()))).midnight();
    (start, start + Duration::days(7))
}

pub fn month(
    at: PrimitiveDateTime,
    subscribed: PrimitiveDateTime,
) -> (PrimitiveDateTime, PrimitiveDateTime) {
    let anchor = |offset: i32| {
        let total = at.year() * 12 + i32::from(u8::from(at.month())) - 1 + offset;
        let year = total.div_euclid(12);
        let month = Month::try_from((total.rem_euclid(12) + 1) as u8).expect("month in range");
        Date::from_calendar_date(year, month, subscribed.day().min(month.length(year)))
            .expect("valid monthly anchor")
            .with_time(subscribed.time())
    };
    if anchor(0) > at {
        return (anchor(-1), anchor(0));
    }
    (anchor(0), anchor(1))
}

#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub limited: bool,
    pub reset: i64,
    pub percent: i64,
}

impl Quota {
    pub fn json(&self, at: PrimitiveDateTime) -> serde_json::Value {
        let reset = at + Duration::seconds(self.reset);
        serde_json::json!({"status":if self.limited {"rate-limited"} else {"ok"},"percent":self.percent,"resetsAt":format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",reset.year(),u8::from(reset.month()),reset.day(),reset.hour(),reset.minute(),reset.second(),reset.millisecond())})
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"status":if self.limited {"rate-limited"} else {"ok"},"resetInSec":self.reset,"usagePercent":self.percent})
    }
}

pub fn analyze(
    limit_dollars: i64,
    usage: Option<i64>,
    updated: Option<PrimitiveDateTime>,
    start: PrimitiveDateTime,
    end: PrimitiveDateTime,
    at: PrimitiveDateTime,
) -> Quota {
    let active = updated.is_some_and(|updated| updated >= start);
    let usage = if active { usage.unwrap_or_default() } else { 0 };
    let limit = i128::from(limit_dollars) * 100_000_000;
    Quota {
        limited: active && i128::from(usage) >= limit,
        percent: if !active {
            0
        } else if limit == 0 {
            100
        } else {
            ((i128::from(usage) * 100).div_euclid(limit).min(100)) as i64
        },
        reset: ((end - at).whole_nanoseconds() + 999_999_999).div_euclid(1_000_000_000) as i64,
    }
}

pub fn rolling(
    limit_dollars: i64,
    hours: i64,
    usage: Option<i64>,
    updated: Option<PrimitiveDateTime>,
    at: PrimitiveDateTime,
) -> Quota {
    let start = at - Duration::hours(hours);
    if updated.is_some_and(|updated| updated < start) {
        return Quota {
            limited: false,
            percent: 0,
            reset: hours * 3600,
        };
    }
    analyze(
        limit_dollars,
        usage,
        updated.or(Some(at)),
        start,
        updated.unwrap_or(at) + Duration::hours(hours),
        at,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Lite,
    Subscription,
}

impl Plan {
    pub fn parse(value: &str) -> Result<Self, ConsoleError> {
        match value {
            "lite" => Ok(Self::Lite),
            "subscription" => Ok(Self::Subscription),
            _ => Err(ConsoleError::Validation(format!("Unknown plan: {value}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCounters {
    pub rolling_usage: MicroCents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_usage: Option<MicroCents>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_usage: Option<MicroCents>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_usage: Option<MicroCents>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetResult {
    pub plan: Plan,
    pub before: UsageCounters,
}

pub fn sum_lite(
    rows: &[(Option<MicroCents>, Option<MicroCents>, Option<MicroCents>)],
) -> Option<UsageCounters> {
    (!rows.is_empty()).then(|| UsageCounters {
        rolling_usage: rows.iter().map(|r| r.0.unwrap_or_default()).sum(),
        weekly_usage: Some(rows.iter().map(|r| r.1.unwrap_or_default()).sum()),
        monthly_usage: Some(rows.iter().map(|r| r.2.unwrap_or_default()).sum()),
        fixed_usage: None,
    })
}

pub fn sum_subscription(
    rows: &[(Option<MicroCents>, Option<MicroCents>)],
) -> Option<UsageCounters> {
    (!rows.is_empty()).then(|| UsageCounters {
        rolling_usage: rows.iter().map(|r| r.0.unwrap_or_default()).sum(),
        weekly_usage: None,
        monthly_usage: None,
        fixed_usage: Some(rows.iter().map(|r| r.1.unwrap_or_default()).sum()),
    })
}

pub fn missing_rows(plan: Plan) -> ConsoleError {
    ConsoleError::NotFound(
        match plan {
            Plan::Lite => "No lite usage counters found for workspace",
            Plan::Subscription => "No subscription usage counters found for workspace",
        }
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_windows_keep_inclusive_cutoffs_and_month_end_anchors() {
        let at = Date::from_calendar_date(2024, Month::February, 29)
            .unwrap()
            .with_hms_milli(12, 30, 0, 123)
            .unwrap();
        let subscribed = Date::from_calendar_date(2024, Month::January, 31)
            .unwrap()
            .with_hms_milli(12, 30, 0, 123)
            .unwrap();
        assert_eq!(
            month(at, subscribed),
            (
                at,
                Date::from_calendar_date(2024, Month::March, 31)
                    .unwrap()
                    .with_time(subscribed.time())
            )
        );
        assert_eq!(
            month(at - Duration::milliseconds(1), subscribed).0,
            subscribed
        );
        let cutoff = at - Duration::hours(5);
        assert_eq!(rolling(1, 5, Some(-1), Some(at), at).percent, -1);
        assert!(rolling(1, 5, Some(100_000_000), Some(cutoff), at).limited);
        let expired = rolling(
            0,
            5,
            Some(100_000_000),
            Some(cutoff - Duration::milliseconds(1)),
            at,
        );
        assert!(!expired.limited);
        assert_eq!((expired.percent, expired.reset), (0, 18_000));
        let week = week(at);
        assert_eq!(week.0.weekday(), time::Weekday::Monday);
        assert_eq!(week.0.hour(), 0);
        assert!(
            !analyze(
                0,
                Some(100),
                Some(week.0 - Duration::milliseconds(1)),
                week.0,
                week.1,
                at
            )
            .limited
        );
    }

    #[test]
    fn reset_results_sum_null_counters_and_reject_missing_plan() {
        assert_eq!(
            sum_lite(&[(Some(3), None, Some(9)), (None, Some(4), Some(1))]),
            Some(UsageCounters {
                rolling_usage: 3,
                weekly_usage: Some(4),
                monthly_usage: Some(10),
                fixed_usage: None,
            })
        );
        assert_eq!(sum_lite(&[]), None);
        assert_eq!(
            Plan::parse("unknown").unwrap_err().to_string(),
            "Unknown plan: unknown"
        );
        assert_eq!(
            missing_rows(Plan::Lite).to_string(),
            "No lite usage counters found for workspace"
        );
    }
}
