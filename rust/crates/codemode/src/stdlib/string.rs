//! Purpose: `String` methods and statics with JS UTF-16 semantics.
//! Scope: runtime.ts invokeStringMethod (lines 330-520) and stdlib/string.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/stdlib/regexp.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! |-- codemode/src/value/number.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- char::from_u32
//! |-- f64::NAN
//! |-- f64::from
//! |-- str::to_string
//! |-- u32::MAX
//! |-- u32::from
//! `-- unicode_normalization::UnicodeNormalization
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/members.rs
//! `-- codemode/src/stdlib/mod.rs
//! Invariants: argument types are validated exactly as the baseline; results are bounded data.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

use super::regexp::{all_matches, exec_at, host_regex, match_value, replace};
use crate::ast::Loc;
use crate::error::{Eval, fail, fail_named};
use crate::value::Value;
use crate::value::convert::bounded;
use crate::value::number::to_uint32;
use crate::value::text::{
    clamp_index, from_utf16, index_of, is_js_whitespace, last_index_of, len16, locale_compare,
    relative_index, slice16, to_lower, to_upper, utf16,
};
use unicode_normalization::UnicodeNormalization;

pub(crate) const METHODS: &[&str] = &[
    "toLowerCase",
    "toUpperCase",
    "trim",
    "trimStart",
    "trimEnd",
    "trimLeft",
    "trimRight",
    "split",
    "slice",
    "substring",
    "substr",
    "includes",
    "startsWith",
    "endsWith",
    "indexOf",
    "lastIndexOf",
    "replace",
    "replaceAll",
    "repeat",
    "padStart",
    "padEnd",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "at",
    "concat",
    "toString",
    "match",
    "matchAll",
    "search",
    "localeCompare",
    "normalize",
];

pub(crate) const STATICS: &[&str] = &["fromCharCode", "fromCodePoint"];

struct Args<'a> {
    name: &'a str,
    args: &'a [Value],
    loc: Loc,
}

impl Args<'_> {
    fn str(&self, index: usize) -> Eval<String> {
        match self.args.get(index) {
            Some(Value::Str(text)) => Ok(text.to_string()),
            _ => Err(fail(
                format!(
                    "String.{} expects argument {} to be a string.",
                    self.name,
                    index + 1
                ),
                self.loc,
            )),
        }
    }
    fn num(&self, index: usize) -> Eval<f64> {
        match self.args.get(index) {
            Some(Value::Number(number)) => Ok(*number),
            _ => Err(fail(
                format!(
                    "String.{} expects argument {} to be a number.",
                    self.name,
                    index + 1
                ),
                self.loc,
            )),
        }
    }
    fn opt_num(&self, index: usize) -> Eval<Option<f64>> {
        match self.args.get(index) {
            None | Some(Value::Undefined) => Ok(None),
            _ => self.num(index).map(Some),
        }
    }
    fn opt_str(&self, index: usize) -> Eval<Option<String>> {
        match self.args.get(index) {
            None | Some(Value::Undefined) => Ok(None),
            _ => self.str(index).map(Some),
        }
    }
}

fn pad(value: &str, target: f64, filler: Option<String>, start: bool) -> String {
    let filler = filler.unwrap_or_else(|| " ".into());
    let length = len16(value);
    let target = if target.is_nan() {
        0
    } else {
        target.clamp(0.0, 1e9) as usize
    };
    if target <= length || filler.is_empty() {
        return value.to_string();
    }
    let fill_units: Vec<u16> = utf16(&filler)
        .into_iter()
        .cycle()
        .take(target - length)
        .collect();
    let fill = from_utf16(&fill_units);
    if start {
        format!("{fill}{value}")
    } else {
        format!("{value}{fill}")
    }
}

pub(crate) fn invoke_method(value: &str, name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    let a = Args { name, args, loc };
    let len = len16(value);
    let result = match name {
        "toLowerCase" => Value::str(to_lower(value)),
        "toUpperCase" => Value::str(to_upper(value)),
        "trim" => Value::str(value.trim_matches(is_js_whitespace)),
        "trimStart" | "trimLeft" => Value::str(value.trim_start_matches(is_js_whitespace)),
        "trimEnd" | "trimRight" => Value::str(value.trim_end_matches(is_js_whitespace)),
        "localeCompare" => Value::Number(match locale_compare(value, &a.str(0)?) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        }),
        "normalize" => {
            let form = a.opt_str(0)?;
            Value::str(match form.as_deref().unwrap_or("NFC") {
                "NFC" => value.nfc().collect::<String>(),
                "NFD" => value.nfd().collect(),
                "NFKC" => value.nfkc().collect(),
                "NFKD" => value.nfkd().collect(),
                _ => {
                    let rendered = form.map_or_else(
                        || "undefined".into(),
                        |form| crate::value::text::quote_json(&form),
                    );
                    return Err(fail_named(
                        format!(
                            "String.normalize expects the form \"NFC\", \"NFD\", \"NFKC\", or \"NFKD\" (got {rendered})."
                        ),
                        loc,
                        "RangeError",
                    ));
                }
            })
        }
        "split" => split(value, &a)?,
        "slice" => {
            let start = relative_index(a.opt_num(0)?.unwrap_or(0.0), len);
            let end = a.opt_num(1)?.map_or(len, |end| relative_index(end, len));
            Value::str(slice16(value, start, end))
        }
        "includes" => {
            let from = clamp_index(a.opt_num(1)?.unwrap_or(0.0), len);
            Value::Bool(index_of(value, &a.str(0)?, from).is_some())
        }
        "startsWith" => {
            let from = clamp_index(a.opt_num(1)?.unwrap_or(0.0), len);
            let needle = utf16(&a.str(0)?);
            let units = utf16(value);
            Value::Bool(
                units
                    .get(from..)
                    .is_some_and(|rest| rest.starts_with(&needle)),
            )
        }
        "endsWith" => {
            let end = a.opt_num(1)?.map_or(len, |end| clamp_index(end, len));
            let needle = utf16(&a.str(0)?);
            let units = utf16(value);
            Value::Bool(units[..end].ends_with(&needle))
        }
        "indexOf" => {
            let from = clamp_index(a.opt_num(1)?.unwrap_or(0.0), len);
            Value::Number(index_of(value, &a.str(0)?, from).map_or(-1.0, |index| index as f64))
        }
        "lastIndexOf" => {
            let from = a.opt_num(1)?.map_or(len, |from| {
                if from.is_nan() {
                    len
                } else {
                    clamp_index(from, len)
                }
            });
            Value::Number(last_index_of(value, &a.str(0)?, from).map_or(-1.0, |index| index as f64))
        }
        "replace" | "replaceAll" => {
            let pattern = args.first().cloned().unwrap_or(Value::Undefined);
            if !matches!(pattern, Value::RegExp(_)) {
                a.str(0)?;
            }
            Value::str(replace(value, name, &pattern, &a.str(1)?, loc)?)
        }
        "match" => {
            let regex = host_regex(args.first().unwrap_or(&Value::Undefined), name, loc, "")?;
            let units = utf16(value);
            if regex.has('g') {
                let found = all_matches(&regex, &units);
                *crate::value::lock(&regex.last_index) = 0.0;
                if found.is_empty() {
                    return Ok(Value::Null);
                }
                let items = found
                    .iter()
                    .map(|item| Value::str(from_utf16(&units[item.start..item.end])))
                    .collect();
                return bounded(&Value::array(items), "String.match result");
            }
            return Ok(
                exec_at(&regex, &units, 0).map_or(Value::Null, |found| match_value(&units, &found))
            );
        }
        "matchAll" => {
            let regex = host_regex(args.first().unwrap_or(&Value::Undefined), name, loc, "g")?;
            if !regex.has('g') {
                return Err(fail(
                    format!(
                        "String.matchAll requires a regular expression with the global (g) flag: write /{}/{}g, or use String.match for a single match.",
                        regex.source, regex.flags
                    ),
                    loc,
                ));
            }
            let units = utf16(value);
            return Ok(Value::array(
                all_matches(&regex, &units)
                    .iter()
                    .map(|found| match_value(&units, found))
                    .collect(),
            ));
        }
        "search" => {
            let regex = host_regex(args.first().unwrap_or(&Value::Undefined), name, loc, "")?;
            let units = utf16(value);
            let without_sticky = regex.flags.replace('y', "");
            let found = super::regexp::compile(&regex.source.replace("(?:)", ""), &without_sticky)
                .ok()
                .and_then(|plain| exec_at(&plain, &units, 0));
            Value::Number(found.map_or(-1.0, |found| found.start as f64))
        }
        "repeat" => {
            let count = a.num(0)?;
            if !count.is_finite() || count < 0.0 {
                return Err(fail(
                    "String.repeat expects a finite non-negative count.",
                    loc,
                ));
            }
            Value::str(value.repeat(count as usize))
        }
        "padStart" | "padEnd" => {
            Value::str(pad(value, a.num(0)?, a.opt_str(1)?, name == "padStart"))
        }
        "charAt" => {
            let index = a.opt_num(0)?.unwrap_or(0.0).trunc();
            let units = utf16(value);
            Value::str(if index >= 0.0 {
                units
                    .get(index as usize)
                    .map_or(String::new(), |unit| from_utf16(&[*unit]))
            } else {
                String::new()
            })
        }
        "at" => {
            let index = a.opt_num(0)?.unwrap_or(0.0).trunc();
            let units = utf16(value);
            let resolved = if index < 0.0 {
                units.len() as f64 + index
            } else {
                index
            };
            if resolved < 0.0 || resolved >= units.len() as f64 {
                Value::Undefined
            } else {
                Value::str(from_utf16(&[units[resolved as usize]]))
            }
        }
        "substring" => {
            let start = clamp_index(a.opt_num(0)?.unwrap_or(0.0), len);
            let end = a.opt_num(1)?.map_or(len, |end| clamp_index(end, len));
            Value::str(slice16(value, start.min(end), start.max(end)))
        }
        "substr" => {
            let start = relative_index(a.opt_num(0)?.unwrap_or(0.0), len);
            let count =
                a.opt_num(1)?.map_or(
                    len as f64,
                    |count| if count.is_nan() { 0.0 } else { count.trunc() },
                );
            let end = (start as f64 + count.max(0.0)).min(len as f64) as usize;
            Value::str(slice16(value, start, end))
        }
        "charCodeAt" => {
            let index = a.opt_num(0)?.unwrap_or(0.0).trunc();
            let units = utf16(value);
            Value::Number(if index >= 0.0 {
                units
                    .get(index as usize)
                    .map_or(f64::NAN, |unit| f64::from(*unit))
            } else {
                f64::NAN
            })
        }
        "codePointAt" => {
            let index = a.opt_num(0)?.unwrap_or(0.0).trunc();
            let units = utf16(value);
            if index < 0.0 || index as usize >= units.len() {
                Value::Undefined
            } else {
                let position = index as usize;
                let first = units[position];
                let code = if (0xD800..0xDC00).contains(&first)
                    && let Some(second) = units
                        .get(position + 1)
                        .filter(|second| (0xDC00..0xE000).contains(*second))
                {
                    0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(*second) - 0xDC00)
                } else {
                    u32::from(first)
                };
                Value::Number(f64::from(code))
            }
        }
        "toString" => Value::str(value),
        "concat" => {
            let mut out = value.to_string();
            for index in 0..args.len() {
                out.push_str(&a.str(index)?);
            }
            Value::str(out)
        }
        _ => {
            return Err(fail(
                format!("String method '{name}' is not available in CodeMode."),
                loc,
            ));
        }
    };
    bounded(&result, &format!("String.{name} result"))
}

fn split(value: &str, a: &Args<'_>) -> Eval<Value> {
    if a.args.is_empty() {
        return Ok(Value::array(vec![Value::str(value)]));
    }
    let limit = a
        .opt_num(1)?
        .map_or(u32::MAX as usize, |limit| to_uint32(limit) as usize);
    let units = utf16(value);
    let mut parts: Vec<String> = Vec::new();
    if let Some(Value::RegExp(regex)) = a.args.first() {
        if units.is_empty() {
            return Ok(Value::array(
                if exec_at(regex, &units, 0).is_some_and(|found| found.end == 0) {
                    vec![]
                } else {
                    vec![Value::str("")]
                },
            ));
        }
        // ECMAScript @@split: p is the end of the last split, q the next search position.
        let mut p = 0;
        let mut q = 0;
        while q < units.len() {
            let Some(found) = exec_at(regex, &units, q) else {
                break;
            };
            if found.start >= units.len() {
                break;
            }
            q = found.start;
            if found.end == p {
                q += 1;
                continue;
            }
            parts.push(from_utf16(&units[p..q]));
            for group in &found.groups {
                parts.push(
                    group.map_or(String::new(), |range| from_utf16(&units[range.0..range.1])),
                );
            }
            p = found.end;
            q = p;
        }
        parts.push(from_utf16(&units[p..]));
    } else {
        let separator = a.str(0)?;
        if separator.is_empty() {
            parts = units.iter().map(|unit| from_utf16(&[*unit])).collect();
        } else {
            parts = value
                .split(separator.as_str())
                .map(str::to_string)
                .collect();
        }
    }
    Ok(Value::array(
        parts.into_iter().take(limit).map(Value::str).collect(),
    ))
}

pub(crate) fn invoke_static(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    let codes = args
        .iter()
        .map(|arg| match arg {
            Value::Number(number) => Ok(*number),
            _ => Err(fail(
                format!("String.{name} expects number arguments."),
                loc,
            )),
        })
        .collect::<Eval<Vec<f64>>>()?;
    match name {
        "fromCharCode" => {
            let units: Vec<u16> = codes.iter().map(|code| to_uint32(*code) as u16).collect();
            Ok(Value::str(from_utf16(&units)))
        }
        "fromCodePoint" => {
            let mut out = String::new();
            for code in codes {
                let valid = code.fract() == 0.0 && (0.0..=1_114_111.0).contains(&code);
                match char::from_u32(code as u32).filter(|_| valid) {
                    Some(ch) => out.push(ch),
                    None if valid => out.push('\u{FFFD}'),
                    None => {
                        return Err(fail_named(
                            format!("Invalid code point {}", crate::value::number::format(code)),
                            loc,
                            "RangeError",
                        ));
                    }
                }
            }
            Ok(Value::str(out))
        }
        _ => Err(fail(
            format!("String.{name} is not available in CodeMode."),
            loc,
        )),
    }
}
