//! Purpose: `RegExp` values over `regress` with JS UTF-16 indexing, `lastIndex` semantics,
//! match arrays, replacement templates, and the string pattern operations built on them.
//! Scope: stdlib/regexp.ts plus the regex paths of runtime.ts invokeStringMethod/replacers.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/lib.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- regress::Flags::from
//! `-- regress::Regex::with_flags
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/stdlib/mod.rs
//! `-- codemode/src/stdlib/string.rs
//! Invariants: all offsets are UTF-16 code units; named groups never expose blocked keys.
//! Contract tests: codemode/tests/stdlib.rs, codemode/tests/parity.rs

use crate::ast::Loc;
use crate::error::{Eval, fail, fail_named};
use crate::value::convert::{is_blocked_member, to_number, to_string};
use crate::value::text::{byte_offset, from_utf16, quote_json, utf16};
use crate::value::{ArrayData, ObjectData, RegExpData, Value, lock, shared};
use std::sync::{Arc, Mutex};

pub(crate) const METHODS: &[&str] = &["test", "exec", "toString"];

pub(crate) const ESCAPE_HINT: &str = "To match special characters like ( ) [ ] { } + * ? . literally, escape them with a backslash (e.g. \"\\\\(\") or test for them with String.includes instead.";

const FLAG_ORDER: &str = "dgimsuvy";

/// Compiles `pattern`/`flags`, returning the engine's failure reason on error.
pub(crate) fn compile(pattern: &str, flags: &str) -> Result<RegExpData, String> {
    let mut seen = String::new();
    for flag in flags.chars() {
        if !FLAG_ORDER.contains(flag) || seen.contains(flag) {
            return Err(format!(
                "Invalid flags supplied to RegExp constructor '{flags}'"
            ));
        }
        seen.push(flag);
    }
    if seen.contains('u') && seen.contains('v') {
        return Err(format!(
            "Invalid flags supplied to RegExp constructor '{flags}'"
        ));
    }
    let canonical: String = FLAG_ORDER
        .chars()
        .filter(|flag| seen.contains(*flag))
        .collect();
    let regex = regress::Regex::with_flags(pattern, regress::Flags::from(canonical.as_str()))
        .map_err(|error| error.to_string())?;
    Ok(RegExpData {
        source: if pattern.is_empty() {
            "(?:)".into()
        } else {
            pattern.to_string()
        },
        flags: canonical,
        regex,
        last_index: Mutex::new(0.0),
    })
}

/// `new RegExp(pattern?, flags?)` and regex literals.
pub(crate) fn construct(args: &[Value], loc: Loc) -> Eval<Value> {
    let first = args.first().cloned().unwrap_or(Value::Undefined);
    let pattern = match &first {
        Value::RegExp(regex) => regex.source.clone(),
        Value::Undefined => String::new(),
        other => to_string(other),
    };
    let flags = match args.get(1) {
        None | Some(Value::Undefined) => match &first {
            Value::RegExp(regex) => regex.flags.clone(),
            _ => String::new(),
        },
        Some(Value::Str(flags)) => flags.to_string(),
        Some(other) => {
            let kind = if matches!(other, Value::Null) {
                "null"
            } else {
                crate::interp_typeof(other)
            };
            return Err(fail(
                format!(
                    "RegExp flags must be a string of flag characters (e.g. \"g\", \"gi\"), not {kind}."
                ),
                loc,
            ));
        }
    };
    let pattern = if pattern == "(?:)" {
        String::new()
    } else {
        pattern
    };
    match compile(&pattern, &flags) {
        Ok(data) => Ok(Value::RegExp(Arc::new(data))),
        Err(reason) => {
            let reason = reason
                .trim_start_matches("Invalid regular expression: ")
                .to_string();
            let message = if reason.to_lowercase().contains("flag") {
                format!(
                    "new RegExp(...) received invalid flags {} ({reason}). Valid flags are d, g, i, m, s, u, v, and y.",
                    quote_json(&flags)
                )
            } else {
                format!(
                    "new RegExp(...) received {}, which is not a valid regular expression pattern ({reason}). {ESCAPE_HINT}",
                    quote_json(&pattern)
                )
            };
            Err(fail_named(message, loc, "SyntaxError"))
        }
    }
}

/// `toHostRegex`: a regex argument, or a string compiled as a pattern.
pub(crate) fn host_regex(
    arg: &Value,
    method: &str,
    loc: Loc,
    extra_flags: &str,
) -> Eval<Arc<RegExpData>> {
    match arg {
        Value::RegExp(regex) => Ok(regex.clone()),
        Value::Str(pattern) => compile(pattern, extra_flags).map(Arc::new).map_err(|reason| {
            fail_named(
                format!(
                    "String.{method} received the string {}, which is not a valid regular expression pattern ({reason}). {ESCAPE_HINT}",
                    quote_json(pattern)
                ),
                loc,
                "SyntaxError",
            )
        }),
        other => {
            let kind = if matches!(other, Value::Null) { "null" } else { crate::interp_typeof(other) };
            Err(fail(
                format!(
                    "String.{method} expects a regular expression (a /pattern/flags literal or new RegExp(...)) or a string pattern, not {kind}."
                ),
                loc,
            ))
        }
    }
}

pub(crate) fn property(regex: &RegExpData, name: &str) -> Option<Value> {
    Some(match name {
        "source" => Value::str(&regex.source),
        "flags" => Value::str(&regex.flags),
        "lastIndex" => Value::Number(*lock(&regex.last_index)),
        "global" => Value::Bool(regex.has('g')),
        "ignoreCase" => Value::Bool(regex.has('i')),
        "multiline" => Value::Bool(regex.has('m')),
        "sticky" => Value::Bool(regex.has('y')),
        "unicode" => Value::Bool(regex.has('u')),
        "dotAll" => Value::Bool(regex.has('s')),
        _ => return None,
    })
}

/// One match in UTF-16 units: whole range, capture ranges, named group names.
pub(crate) struct Found {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
    pub names: Vec<(String, Option<(usize, usize)>)>,
}

/// Matches at or after `start` (exactly at `start` for sticky regexes).
pub(crate) fn exec_at(regex: &RegExpData, units: &[u16], start: usize) -> Option<Found> {
    if start > units.len() {
        return None;
    }
    let found = regex.regex.find_from_utf16(units, start).next()?;
    if regex.has('y') && found.start() != start {
        return None;
    }
    let groups = found
        .groups()
        .skip(1)
        .map(|range| range.map(|range| (range.start, range.end)))
        .collect();
    let names = found
        .named_groups()
        .map(|(name, range)| {
            (
                name.to_string(),
                range.map(|range| (range.start, range.end)),
            )
        })
        .collect();
    Some(Found {
        start: found.start(),
        end: found.end(),
        groups,
        names,
    })
}

/// Every match from position 0, advancing past empty matches (by code point in unicode mode).
pub(crate) fn all_matches(regex: &RegExpData, units: &[u16]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut position = 0;
    while let Some(found) = exec_at(regex, units, position) {
        position = if found.end == found.start {
            advance(regex, units, found.end)
        } else {
            found.end
        };
        out.push(found);
        if position > units.len() {
            break;
        }
    }
    out
}

fn advance(regex: &RegExpData, units: &[u16], index: usize) -> usize {
    let unicode = regex.has('u') || regex.has('v');
    if unicode
        && index + 1 < units.len()
        && (0xD800..0xDC00).contains(&units[index])
        && (0xDC00..0xE000).contains(&units[index + 1])
    {
        index + 2
    } else {
        index + 1
    }
}

fn slice(units: &[u16], range: (usize, usize)) -> String {
    from_utf16(&units[range.0..range.1])
}

/// A match as a sandbox array with `index` (and `groups` when named groups exist).
pub(crate) fn match_value(units: &[u16], found: &Found) -> Value {
    let mut items = vec![Value::str(slice(units, (found.start, found.end)))];
    items.extend(
        found
            .groups
            .iter()
            .map(|range| range.map_or(Value::Undefined, |range| Value::str(slice(units, range)))),
    );
    let mut data = ArrayData::new(items);
    data.extra
        .insert("index".into(), Value::Number(found.start as f64));
    if !found.names.is_empty() {
        data.extra
            .insert("groups".into(), groups_value(units, found));
    }
    Value::Array(shared(data))
}

fn groups_value(units: &[u16], found: &Found) -> Value {
    Value::object(ObjectData::from_pairs(
        found
            .names
            .iter()
            .filter(|(name, _)| !is_blocked_member(name))
            .map(|(name, range)| {
                (
                    name.clone(),
                    range.map_or(Value::Undefined, |range| Value::str(slice(units, range))),
                )
            }),
    ))
}

pub(crate) fn invoke_method(
    regex: &Arc<RegExpData>,
    name: &str,
    args: &[Value],
    loc: Loc,
) -> Eval<Value> {
    match name {
        "test" | "exec" => {
            let text = to_string(args.first().unwrap_or(&Value::Undefined));
            let units = utf16(&text);
            let stateful = regex.has('g') || regex.has('y');
            let start = if stateful {
                to_number(&Value::Number(*lock(&regex.last_index))).max(0.0) as usize
            } else {
                0
            };
            let found = exec_at(regex, &units, start);
            if stateful {
                *lock(&regex.last_index) = found.as_ref().map_or(0.0, |found| found.end as f64);
            }
            Ok(match (name, found) {
                ("test", found) => Value::Bool(found.is_some()),
                (_, Some(found)) => match_value(&units, &found),
                (_, None) => Value::Null,
            })
        }
        "toString" => Ok(Value::str(format!("/{}/{}", regex.source, regex.flags))),
        _ => Err(fail(
            format!("RegExp method '{name}' is not available in CodeMode."),
            loc,
        )),
    }
}

/// Expands a replacement template (`$$`, `$&`, `` $` ``, `$'`, `$n`, `$nn`, `$<name>`).
fn expand(template: &str, units: &[u16], found: &Found) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    let group = |number: usize| {
        found
            .groups
            .get(number - 1)
            .map(|range| range.map_or(String::new(), |range| slice(units, range)))
    };
    while index < chars.len() {
        let ch = chars[index];
        if ch != '$' || index + 1 >= chars.len() {
            out.push(ch);
            index += 1;
            continue;
        }
        let next = chars[index + 1];
        match next {
            '$' => out.push('$'),
            '&' => out.push_str(&slice(units, (found.start, found.end))),
            '`' => out.push_str(&slice(units, (0, found.start))),
            '\'' => out.push_str(&slice(units, (found.end, units.len()))),
            '<' if !found.names.is_empty() => {
                match chars[index + 2..].iter().position(|ch| *ch == '>') {
                    Some(close) => {
                        let name: String = chars[index + 2..index + 2 + close].iter().collect();
                        if let Some((_, Some(range))) =
                            found.names.iter().find(|(group, _)| *group == name)
                        {
                            out.push_str(&slice(units, *range));
                        }
                        index += close + 3;
                        continue;
                    }
                    None => out.push_str("$<"),
                }
            }
            digit if digit.is_ascii_digit() => {
                let two = chars
                    .get(index + 2)
                    .filter(|ch| ch.is_ascii_digit())
                    .map(|second| {
                        (digit.to_digit(10).unwrap_or(0) * 10 + second.to_digit(10).unwrap_or(0))
                            as usize
                    });
                if let Some(number) =
                    two.filter(|number| *number >= 1 && *number <= found.groups.len())
                {
                    out.push_str(&group(number).unwrap_or_default());
                    index += 3;
                    continue;
                }
                let number = digit.to_digit(10).unwrap_or(0) as usize;
                if number >= 1 && number <= found.groups.len() {
                    out.push_str(&group(number).unwrap_or_default());
                } else {
                    out.push('$');
                    out.push(digit);
                }
            }
            other => {
                out.push('$');
                out.push(other);
            }
        }
        index += 2;
    }
    out
}

/// Literal-string matches (`replace(string, ...)`): first only, or every occurrence.
fn string_matches(units: &[u16], needle: &str, all: bool) -> Vec<Found> {
    let needle = utf16(needle);
    let mut out = Vec::new();
    let mut position = 0;
    while position + needle.len() <= units.len() {
        let Some(offset) = (position..=units.len() - needle.len())
            .find(|start| units[*start..*start + needle.len()] == needle[..])
        else {
            break;
        };
        out.push(Found {
            start: offset,
            end: offset + needle.len(),
            groups: Vec::new(),
            names: Vec::new(),
        });
        if !all {
            break;
        }
        position = offset + needle.len().max(1);
    }
    out
}

fn regex_matches(regex: &RegExpData, units: &[u16], name: &str, loc: Loc) -> Eval<Vec<Found>> {
    if name == "replaceAll" && !regex.has('g') {
        return Err(fail(
            format!(
                "String.replaceAll requires a regular expression with the global (g) flag: write /{}/{}g, or use String.replace to replace only the first match.",
                regex.source, regex.flags
            ),
            loc,
        ));
    }
    if regex.has('g') {
        *lock(&regex.last_index) = 0.0;
        return Ok(all_matches(regex, units));
    }
    Ok(exec_at(regex, units, 0).into_iter().collect())
}

/// `String.replace/replaceAll` with a template replacement.
pub(crate) fn replace(
    text: &str,
    name: &str,
    pattern: &Value,
    replacement: &str,
    loc: Loc,
) -> Eval<String> {
    let units = utf16(text);
    let matches = match pattern {
        Value::RegExp(regex) => regex_matches(regex, &units, name, loc)?,
        other => string_matches(&units, &to_string(other), name == "replaceAll"),
    };
    let mut out: Vec<u16> = Vec::new();
    let mut end = 0;
    for found in &matches {
        out.extend_from_slice(&units[end..found.start]);
        out.extend(utf16(&expand(replacement, &units, found)));
        end = found.end;
    }
    out.extend_from_slice(&units[end..]);
    Ok(from_utf16(&out))
}

/// A match prepared for a function replacer: byte range plus callback arguments.
pub(crate) struct ReplacerMatch {
    pub start: usize,
    pub end: usize,
    pub args: Vec<Value>,
}

pub(crate) fn replacer_matches(
    text: &str,
    name: &str,
    pattern: &Value,
    loc: Loc,
) -> Eval<Vec<ReplacerMatch>> {
    let units = utf16(text);
    let matches = match pattern {
        Value::RegExp(regex) => regex_matches(regex, &units, name, loc)?,
        Value::Str(needle) => string_matches(&units, needle, name == "replaceAll"),
        _ => {
            return Err(fail(
                format!("String.{name} expects argument 1 to be a string."),
                loc,
            ));
        }
    };
    Ok(matches
        .into_iter()
        .map(|found| {
            let mut args = vec![Value::str(slice(&units, (found.start, found.end)))];
            args.extend(found.groups.iter().map(|range| {
                range.map_or(Value::Undefined, |range| Value::str(slice(&units, range)))
            }));
            args.push(Value::Number(found.start as f64));
            args.push(Value::str(text));
            if !found.names.is_empty() {
                args.push(groups_value(&units, &found));
            }
            ReplacerMatch {
                start: byte_offset(text, found.start),
                end: byte_offset(text, found.end),
                args,
            }
        })
        .collect())
}
