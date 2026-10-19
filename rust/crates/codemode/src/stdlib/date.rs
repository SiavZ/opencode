//! Purpose: `Date` values: construction, parsing, ISO rendering, UTC/local getters.
//! Scope: stdlib/date.ts and runtime.ts constructDate (lines 1510-1522).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/clock.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- f64::NAN
//! `-- i64::from
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/stdlib/mod.rs
//! `-- codemode/src/value/convert.rs
//! Invariants: time values are epoch milliseconds clipped to +-8.64e15; local time is UTC (the
//! confined runtime has no ambient timezone, see codemode.md blockers).
//! Contract tests: codemode/tests/stdlib.rs

use crate::ast::Loc;
use crate::error::{Eval, fail};
use crate::value::Value;
use crate::value::convert::{to_number, to_string};
use std::sync::Arc;

pub(crate) const METHODS: &[&str] = &[
    "getTime",
    "valueOf",
    "toISOString",
    "toJSON",
    "toString",
    "getFullYear",
    "getMonth",
    "getDate",
    "getDay",
    "getHours",
    "getMinutes",
    "getSeconds",
    "getMilliseconds",
    "getUTCFullYear",
    "getUTCMonth",
    "getUTCDate",
    "getUTCDay",
    "getUTCHours",
    "getUTCMinutes",
    "getUTCSeconds",
    "getUTCMilliseconds",
    "getTimezoneOffset",
];

const MS_PER_DAY: f64 = 86_400_000.0;

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        f64::NAN
    } else {
        time.trunc() + 0.0
    }
}

/// `MakeDate(MakeDay(year, month, day), MakeTime(h, m, s, ms))`.
pub(crate) fn make_time(parts: &[f64]) -> f64 {
    if parts.iter().any(|part| !part.is_finite()) {
        return f64::NAN;
    }
    let get = |index: usize, default: f64| parts.get(index).map_or(default, |value| value.trunc());
    let year = get(0, f64::NAN);
    let month = get(1, 0.0);
    let year = year + (month / 12.0).floor();
    let month = month.rem_euclid(12.0);
    let days = days_from_civil(year as i64, month as i64 + 1, 1) as f64 + get(2, 1.0) - 1.0;
    let time =
        get(3, 0.0) * 3_600_000.0 + get(4, 0.0) * 60_000.0 + get(5, 0.0) * 1000.0 + get(6, 0.0);
    time_clip(days * MS_PER_DAY + time)
}

pub(crate) fn to_iso(time: f64) -> Option<String> {
    if !time.is_finite() {
        return None;
    }
    let days = (time / MS_PER_DAY).floor();
    let ms_of_day = time - days * MS_PER_DAY;
    let (year, month, day) = civil_from_days(days as i64);
    let ms = ms_of_day as i64;
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    Some(format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    ))
}

/// `Date.parse` for the ISO formats plus RFC-2822-style `Mon DD YYYY` dates JS engines accept.
pub(crate) fn parse(text: &str) -> f64 {
    parse_iso(text.trim()).unwrap_or(f64::NAN)
}

fn parse_iso(text: &str) -> Option<f64> {
    let (date, rest) = match text.find('T').or_else(|| text.find(' ')) {
        Some(index) => (&text[..index], Some(&text[index + 1..])),
        None => (text, None),
    };
    let (sign, digits) = match date.strip_prefix('+') {
        Some(rest) => (1, rest),
        None => match date.strip_prefix('-') {
            Some(rest) if date.len() > 7 && rest.len() >= 6 && rest.as_bytes()[6] == b'-' => {
                (-1, rest)
            }
            _ => (1, date),
        },
    };
    let mut fields = digits.split('-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: i64 = fields.next().map_or(Some(1), |value| value.parse().ok())?;
    let day: i64 = fields.next().map_or(Some(1), |value| value.parse().ok())?;
    if fields.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut hour, mut minute, mut second, mut millis, mut offset) = (0.0, 0.0, 0.0, 0.0, 0.0);
    if let Some(rest) = rest {
        let (clock, zone) = match rest.find(['Z', 'z', '+', '-']) {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        let mut parts = clock.split(':');
        hour = parts.next()?.parse().ok()?;
        minute = parts.next()?.parse().ok()?;
        if let Some(seconds) = parts.next() {
            let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
            second = whole.parse().ok()?;
            if !fraction.is_empty() {
                millis = format!("0.{fraction}").parse::<f64>().ok()? * 1000.0;
            }
        }
        if !zone.is_empty() && !zone.eq_ignore_ascii_case("z") {
            let negative = zone.starts_with('-');
            let (zone_hours, zone_minutes) = zone[1..]
                .split_once(':')
                .unwrap_or((&zone[1..3.min(zone.len())], &zone[3.min(zone.len())..]));
            let total =
                zone_hours.parse::<f64>().ok()? * 60.0 + zone_minutes.parse::<f64>().unwrap_or(0.0);
            offset = if negative { -total } else { total };
        }
    }
    let time = make_time(&[
        (sign * year) as f64,
        (month - 1) as f64,
        day as f64,
        hour,
        minute,
        second,
        millis.trunc(),
    ]);
    Some(time - offset * 60_000.0)
}

/// `new Date(...)`.
pub(crate) fn construct(args: &[Value]) -> Value {
    let time = match args {
        [] => crate::clock::now_ms(),
        [Value::Date(time)] => **time,
        [Value::Number(number)] => time_clip(*number),
        [Value::Str(text)] => parse(text),
        [_] => f64::NAN,
        parts => make_time(&parts.iter().map(to_number).collect::<Vec<_>>()),
    };
    Value::Date(Arc::new(time))
}

pub(crate) fn invoke_static(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    Ok(Value::Number(match name {
        "now" => crate::clock::now_ms(),
        "parse" => parse(&to_string(args.first().unwrap_or(&Value::Undefined))),
        "UTC" => make_time(&args.iter().map(to_number).collect::<Vec<_>>()),
        _ => {
            return Err(fail(
                format!("Date.{name} is not available in CodeMode."),
                loc,
            ));
        }
    }))
}

pub(crate) fn invoke_method(time: f64, name: &str, loc: Loc) -> Eval<Value> {
    if name == "toISOString" {
        return to_iso(time)
            .map(Value::str)
            .ok_or_else(|| fail("Invalid time value.", loc));
    }
    if name == "toJSON" {
        return Ok(to_iso(time).map_or(Value::Null, Value::str));
    }
    if name == "toString" {
        return Ok(Value::str(to_string(&Value::Date(Arc::new(time)))));
    }
    if matches!(name, "getTime" | "valueOf") {
        return Ok(Value::Number(time));
    }
    if name == "getTimezoneOffset" {
        return Ok(Value::Number(if time.is_finite() { 0.0 } else { f64::NAN }));
    }
    if !METHODS.contains(&name) {
        return Err(fail(
            format!("Date method '{name}' is not available in CodeMode."),
            loc,
        ));
    }
    if !time.is_finite() {
        return Ok(Value::Number(f64::NAN));
    }
    let days = (time / MS_PER_DAY).floor();
    let ms = (time - days * MS_PER_DAY) as i64;
    let (year, month, day) = civil_from_days(days as i64);
    let field = name.trim_start_matches("get").trim_start_matches("UTC");
    Ok(Value::Number(match field {
        "FullYear" => year as f64,
        "Month" => (month - 1) as f64,
        "Date" => day as f64,
        "Day" => (days as i64 + 4).rem_euclid(7) as f64,
        "Hours" => (ms / 3_600_000) as f64,
        "Minutes" => (ms / 60_000 % 60) as f64,
        "Seconds" => (ms / 1000 % 60) as f64,
        _ => (ms % 1000) as f64,
    }))
}
