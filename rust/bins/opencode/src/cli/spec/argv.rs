//! Purpose: yargs-parser 22 compatible option/positional tokenizer for one resolved command:
//! long/short/grouped switches, `=` values, boolean negation, arrays, number coercion, camelCase keys.
//! Scope: pure function over argv tokens.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/invocation.rs
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! |-- f64::INFINITY
//! |-- f64::NEG_INFINITY
//! `-- i64::from_str_radix
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! Invariants: unknown switches are reported (strict mode), never silently dropped; a value-taking
//! option never swallows a following switch, except negative numbers.
//! Contract tests: opencode/tests/cli_parse.rs
use std::collections::BTreeMap;

use super::invocation::Value;
use super::model::{Kind, Opt};

#[derive(Debug, Default)]
pub struct Tokens {
    pub values: BTreeMap<String, Value>,
    pub positionals: Vec<String>,
    pub unknown: Vec<String>,
    pub help: bool,
    pub version: bool,
}

/// Parses `tokens` (no `--` passthrough, no command words) against `opts`.
pub fn tokenize(tokens: &[String], opts: &[Opt]) -> Tokens {
    let mut out = Tokens::default();
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        i += 1;
        if let Some(body) = tok.strip_prefix("--").filter(|b| !b.is_empty()) {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            // Boolean negation wins over an explicit `no-x` option, as in yargs-parser.
            let negated = name
                .strip_prefix("no-")
                .and_then(|base| lookup(opts, base))
                .filter(|o| o.kind == Kind::Bool && inline.is_none());
            if let Some(opt) = negated {
                set(&mut out, opt, Value::Bool(false));
                continue;
            }
            match lookup(opts, name) {
                Some(opt) => i += take(&mut out, opt, inline, &tokens[i..]),
                None => match name.strip_prefix("no-").and_then(|base| lookup(opts, base)) {
                    Some(opt) if inline.is_none() => set(&mut out, opt, Value::Bool(false)),
                    _ => {
                        out.unknown
                            .push(kebab(name.strip_prefix("no-").unwrap_or(name)));
                        if inline.is_none() && tokens.get(i).is_some_and(|t| !is_switch(t)) {
                            i += 1;
                        }
                    }
                },
            }
            continue;
        }
        if tok.len() > 1 && tok.starts_with('-') && !is_negative_number(tok) {
            i += short(&mut out, opts, &tok[1..], &tokens[i..]);
            continue;
        }
        out.positionals.push(tok.clone());
    }
    out
}

/// Grouped short switches `-abc`, `-n5`, `-m=x`.
fn short(out: &mut Tokens, opts: &[Opt], body: &str, rest: &[String]) -> usize {
    if let Some((name, value)) = body.split_once('=') {
        return match lookup(opts, name) {
            Some(opt) => take(out, opt, Some(value.to_string()), rest),
            None => {
                out.unknown.push(name.to_string());
                0
            }
        };
    }
    let chars: Vec<char> = body.chars().collect();
    for (idx, ch) in chars.iter().enumerate() {
        let name = ch.to_string();
        let tail: String = chars[idx + 1..].iter().collect();
        let last = idx + 1 == chars.len();
        let Some(opt) = lookup(opts, &name) else {
            out.unknown.push(name);
            if last {
                return 0;
            }
            continue;
        };
        if last {
            return take(out, opt, None, rest);
        }
        // `-n5` (numeric remainder) and `-m/x` (non-word next char) keep the remainder as the value.
        let numeric =
            tail.parse::<f64>().is_ok() && tail.chars().all(|c| c.is_ascii_digit() || c == '.');
        let non_word = tail
            .chars()
            .next()
            .is_some_and(|c| !(c.is_alphanumeric() || c == '_'));
        if numeric || non_word {
            take(out, opt, Some(tail), &[]);
            return 0;
        }
        match opt.kind {
            Kind::Bool | Kind::Any => set(out, opt, Value::Bool(true)),
            Kind::Str => set(out, opt, Value::Str(String::new())),
            Kind::Array => set(out, opt, Value::List(Vec::new())),
            Kind::Num => {}
        }
    }
    0
}

/// Applies one option occurrence; returns how many following tokens were consumed.
fn take(out: &mut Tokens, opt: &Opt, inline: Option<String>, rest: &[String]) -> usize {
    let next_value = rest
        .first()
        .filter(|t| !is_switch(t) || is_negative_number(t));
    match opt.kind {
        Kind::Bool => match inline {
            Some(v) => {
                set(out, opt, Value::Bool(v == "true"));
                0
            }
            None => match next_value.map(String::as_str) {
                Some(v @ ("true" | "false")) => {
                    set(out, opt, Value::Bool(v == "true"));
                    1
                }
                _ => {
                    set(out, opt, Value::Bool(true));
                    0
                }
            },
        },
        Kind::Str => {
            let (value, used) = value_or(inline, next_value, String::new());
            set(out, opt, Value::Str(value));
            used
        }
        Kind::Num => match (inline, next_value) {
            (Some(v), _) => {
                set(out, opt, js_number(&v));
                0
            }
            (None, Some(v)) => {
                set(out, opt, js_number(v));
                1
            }
            // `--port` without a value leaves the key unset.
            (None, None) => 0,
        },
        Kind::Any => match (inline, next_value) {
            (Some(v), _) => {
                set(out, opt, number_or_string(&v));
                0
            }
            (None, Some(v)) => {
                set(out, opt, number_or_string(v));
                1
            }
            (None, None) => {
                set(out, opt, Value::Bool(true));
                0
            }
        },
        Kind::Array => {
            let mut items: Vec<Value> = inline.into_iter().map(Value::Str).collect();
            let mut used = 0;
            if items.is_empty() {
                for t in rest {
                    if is_switch(t) && !is_negative_number(t) {
                        break;
                    }
                    items.push(Value::Str(t.clone()));
                    used += 1;
                }
            }
            let entry = out
                .values
                .entry(opt.name.to_string())
                .or_insert_with(|| Value::List(Vec::new()));
            if let Value::List(list) = entry {
                list.extend(items);
            }
            mark(out, opt);
            used
        }
    }
}

fn value_or(inline: Option<String>, next: Option<&String>, empty: String) -> (String, usize) {
    match (inline, next) {
        (Some(v), _) => (v, 0),
        (None, Some(v)) => (v.clone(), 1),
        (None, None) => (empty, 0),
    }
}

fn set(out: &mut Tokens, opt: &Opt, value: Value) {
    mark(out, opt);
    let key = opt.name.to_string();
    let merged = match out.values.remove(&key) {
        None => value,
        // Booleans keep the last value; other repeated scalars accumulate like yargs-parser.
        Some(_) if opt.kind == Kind::Bool => value,
        Some(Value::List(mut items)) if opt.kind != Kind::Array => {
            items.push(value);
            Value::List(items)
        }
        Some(prev) => Value::List(vec![prev, value]),
    };
    out.values.insert(key, merged);
}

fn mark(out: &mut Tokens, opt: &Opt) {
    match opt.name {
        "help" => out.help = true,
        "version" => out.version = true,
        _ => {}
    }
}

fn lookup<'a>(opts: &'a [Opt], name: &str) -> Option<&'a Opt> {
    let name = kebab(name);
    opts.iter()
        .find(|o| o.name == name || o.aliases.iter().any(|a| *a == name))
}

/// camel-case-expansion: `--maxCount` addresses `max-count`.
fn kebab(name: &str) -> String {
    if name.len() == 1 {
        return name.to_string();
    }
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn is_switch(t: &str) -> bool {
    t.len() > 1 && t.starts_with('-')
}

fn is_negative_number(t: &str) -> bool {
    t.strip_prefix('-')
        .is_some_and(|n| !n.is_empty() && n.parse::<f64>().is_ok())
}

/// JS `Number(x)` for CLI input: trimmed decimal, hex/octal/binary prefixes, empty is 0.
pub fn js_number(raw: &str) -> Value {
    let t = raw.trim();
    if t.is_empty() {
        return Value::Num(0.0);
    }
    let radix = [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ];
    for (prefix, base) in radix {
        if let Some(digits) = t.strip_prefix(prefix) {
            return i64::from_str_radix(digits, base)
                .map(|n| Value::Num(n as f64))
                .unwrap_or(Value::Null);
        }
    }
    let valid = t
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    match (valid, t) {
        (_, "Infinity" | "+Infinity") => Value::Num(f64::INFINITY),
        (_, "-Infinity") => Value::Num(f64::NEG_INFINITY),
        (true, _) => t.parse::<f64>().map(Value::Num).unwrap_or(Value::Null),
        (false, _) => Value::Null,
    }
}

/// yargs-parser `maybeCoerceNumber` for untyped options: numbers only when they round-trip safely.
fn number_or_string(raw: &str) -> Value {
    let looks_numeric = raw.starts_with("0x") || raw.starts_with("0X") || {
        let digits = raw.strip_prefix('-').unwrap_or(raw);
        !digits.is_empty()
            && digits.parse::<f64>().is_ok()
            && !(digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0."))
    };
    match looks_numeric.then(|| js_number(raw)) {
        Some(Value::Num(n)) if n.is_finite() && n.abs() <= 9_007_199_254_740_991.0 => Value::Num(n),
        _ => Value::Str(raw.to_string()),
    }
}
