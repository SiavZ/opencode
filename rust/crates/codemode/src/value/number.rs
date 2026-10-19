//! Purpose: ECMAScript number semantics (ToString, ToNumber, parseInt/parseFloat, toFixed...).
//! Scope: pure numeric conversions; no interpreter state.
//! Uses:
//! rust/crates/
//! `-- codemode/src/value/text.rs
//! External:
//! |-- char::is_ascii_digit
//! |-- f64::INFINITY
//! |-- f64::NAN
//! |-- f64::NEG_INFINITY
//! |-- f64::from
//! |-- ryu_js::Buffer::new
//! `-- str::is_empty
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/parse/mod.rs
//! |-- codemode/src/run.rs
//! |-- codemode/src/stdlib/math.rs
//! |-- codemode/src/stdlib/number.rs
//! |-- codemode/src/stdlib/string.rs
//! |-- codemode/src/stdlib/value.rs
//! |-- codemode/src/tools/discovery.rs
//! |-- codemode/src/tools/schema.rs
//! |-- codemode/src/tools/text.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! Invariants: formatting matches `Number.prototype.toString` for radix 10.
//! Contract tests: codemode/tests/parity.rs

use super::text::is_js_whitespace;

/// `Number::toString(10)`.
pub(crate) fn format(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    ryu_js::Buffer::new().format(value).to_string()
}

/// `ToNumber(string)`.
pub(crate) fn parse_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(is_js_whitespace);
    if trimmed.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = trimmed.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |total, ch| {
                total * f64::from(radix) + f64::from(ch.to_digit(radix).unwrap_or(0))
            });
        }
    }
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if unsigned == "Infinity" {
        return if trimmed.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if !is_decimal_literal(unsigned) {
        return f64::NAN;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

fn is_decimal_literal(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => (&text[..index], Some(&text[index + 1..])),
        None => (text, None),
    };
    let mut parts = mantissa.splitn(2, '.');
    let integer = parts.next().unwrap_or("");
    let fraction = parts.next();
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(integer) || !fraction.is_none_or(digits) {
        return false;
    }
    if integer.is_empty() && fraction.is_none_or(str::is_empty) {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        }
    }
}

/// Global `parseInt(string, radix)`.
pub(crate) fn parse_int(text: &str, radix: Option<f64>) -> f64 {
    let trimmed = text.trim_start_matches(is_js_whitespace);
    let (negative, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let mut radix = radix.map_or(0, to_int32);
    let mut rest = rest;
    let mut strip_prefix = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return f64::NAN;
        }
        if radix != 16 {
            strip_prefix = false;
        }
    } else {
        radix = 10;
    }
    if strip_prefix && (rest.starts_with("0x") || rest.starts_with("0X")) {
        rest = &rest[2..];
        radix = 16;
    }
    let radix = radix.unsigned_abs();
    let digits: String = rest.chars().take_while(|ch| ch.is_digit(radix)).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    let value = if radix == 10 {
        digits.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        digits.chars().fold(0.0, |total, ch| {
            total * f64::from(radix) + f64::from(ch.to_digit(radix).unwrap_or(0))
        })
    };
    if negative { -value } else { value }
}

/// Global `parseFloat(string)`.
pub(crate) fn parse_float(text: &str) -> f64 {
    let trimmed = text.trim_start_matches(is_js_whitespace);
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    let sign = if trimmed.starts_with('-') { -1.0 } else { 1.0 };
    if unsigned.starts_with("Infinity") {
        return sign * f64::INFINITY;
    }
    let bytes = unsigned.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        seen_digit = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        let mut cursor = end + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
            seen_digit = true;
        }
        if seen_digit {
            end = cursor;
        }
    }
    if !seen_digit {
        return f64::NAN;
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut cursor = end + 1;
        if cursor < bytes.len() && (bytes[cursor] == b'+' || bytes[cursor] == b'-') {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor > start {
            end = cursor;
        }
    }
    sign * unsigned[..end]
        .trim_end_matches('.')
        .parse::<f64>()
        .unwrap_or(f64::NAN)
}

pub(crate) fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

pub(crate) fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    let modulo = truncated.rem_euclid(4_294_967_296.0);
    modulo as u32
}

/// `Number.prototype.toFixed(digits)`.
pub(crate) fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return format(value);
    }
    if value.abs() >= 1e21 {
        return format(value);
    }
    let rendered = format!("{value:.digits$}");
    if rendered.starts_with('-')
        && rendered[1..]
            .bytes()
            .all(|byte| byte == b'0' || byte == b'.')
    {
        return rendered[1..].to_string();
    }
    rendered
}

/// Decimal digits and exponent of `value` rounded to `precision` significant digits.
fn scientific(value: f64, precision: Option<usize>) -> (bool, String, i32) {
    let negative = value < 0.0;
    let magnitude = value.abs();
    let rendered = match precision {
        Some(precision) => format!("{:.*e}", precision.saturating_sub(1), magnitude),
        None => format!("{magnitude:e}"),
    };
    let (mantissa, exponent) = rendered.split_once('e').unwrap_or((&rendered, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = if precision.is_none() {
        // Shortest round-trip digits come from the JS formatter.
        shortest_digits(magnitude).unwrap_or(digits)
    } else {
        digits
    };
    (negative, digits, exponent.parse().unwrap_or(0))
}

fn shortest_digits(magnitude: f64) -> Option<String> {
    let rendered = format!("{magnitude:e}");
    let (mantissa, _) = rendered.split_once('e')?;
    Some(mantissa.chars().filter(char::is_ascii_digit).collect())
}

/// `Number.prototype.toExponential(digits)`.
pub(crate) fn to_exponential(value: f64, digits: Option<usize>) -> String {
    if !value.is_finite() {
        return format(value);
    }
    let (negative, digits, exponent) = scientific(value, digits.map(|count| count + 1));
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    out.push('e');
    out.push(if exponent < 0 { '-' } else { '+' });
    out.push_str(&exponent.abs().to_string());
    out
}

/// `Number.prototype.toPrecision(precision)`.
pub(crate) fn to_precision(value: f64, precision: usize) -> String {
    if !value.is_finite() {
        return format(value);
    }
    if value == 0.0 {
        return if precision > 1 {
            format!("0.{}", "0".repeat(precision - 1))
        } else {
            "0".into()
        };
    }
    let (negative, digits, exponent) = scientific(value, Some(precision));
    let sign = if negative { "-" } else { "" };
    let precision_i = precision as i32;
    if exponent < -6 || exponent >= precision_i {
        let mut out = format!("{sign}{}", &digits[..1]);
        if precision > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&exponent.abs().to_string());
        return out;
    }
    if exponent == precision_i - 1 {
        return format!("{sign}{digits}");
    }
    if exponent >= 0 {
        let split = (exponent + 1) as usize;
        return format!("{sign}{}.{}", &digits[..split], &digits[split..]);
    }
    format!("{sign}0.{}{digits}", "0".repeat((-(exponent + 1)) as usize))
}

/// `Number.prototype.toString(radix)` for radix != 10.
pub(crate) fn to_radix_string(value: f64, radix: u32) -> String {
    if !value.is_finite() || radix == 10 {
        return format(value);
    }
    let negative = value < 0.0;
    let magnitude = value.abs();
    let mut integer = magnitude.trunc();
    let mut fraction = magnitude - integer;
    let mut integer_digits = Vec::new();
    if integer == 0.0 {
        integer_digits.push('0');
    }
    while integer >= 1.0 {
        let digit = (integer % f64::from(radix)) as u32;
        integer_digits.push(std::char::from_digit(digit, radix).unwrap_or('0'));
        integer = (integer / f64::from(radix)).trunc();
    }
    integer_digits.reverse();
    let mut out: String = integer_digits.into_iter().collect();
    if fraction > 0.0 {
        out.push('.');
        let mut count = 0;
        while fraction > 0.0 && count < 52 {
            fraction *= f64::from(radix);
            let digit = fraction.trunc() as u32;
            out.push(std::char::from_digit(digit, radix).unwrap_or('0'));
            fraction -= f64::from(digit);
            count += 1;
        }
    }
    if negative { format!("-{out}") } else { out }
}

pub(crate) fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value && value.abs() <= 9_007_199_254_740_991.0
}

/// `Math.round` (rounds half toward +Infinity).
pub(crate) fn js_round(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let floor = value.floor();
    let result = if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    if result == 0.0 && value < 0.0 {
        -0.0
    } else {
        result
    }
}
