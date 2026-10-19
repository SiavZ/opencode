//! Purpose: match a normalized baseline fixture value against a raw Rust response value,
//! binding placeholders (`{ses_1}`, `{TS}`, `{CURSOR:...}`) consistently across a replay run.
//! Scope: test-only, pure.
//! Uses:
//! `-- none
//! External:
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! |-- percent_encoding::NON_ALPHANUMERIC
//! |-- percent_encoding::utf8_percent_encode
//! |-- regex::Captures
//! |-- regex::Regex
//! |-- regex::escape
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! |-- server/tests/replay_baseline.rs
//! |-- server/tests/support/replay/mod.rs
//! `-- server/tests/support/replay/run.rs
//! Invariants: placeholder vocabulary equals rust/contracts/phase-06/fixtures/README.md. An id
//! placeholder binds once and must then match the same raw value; literal text outside
//! placeholders matches exactly; object key order is compared (wire order is part of the
//! contract).
//! Contract tests: server/tests/replay_baseline.rs

use std::collections::BTreeMap;

use base64::Engine;
use regex::Regex;
use serde_json::Value;

/// Placeholder bindings shared by every case of one replay run.
#[derive(Debug, Default)]
pub struct Bindings {
    pub ids: BTreeMap<String, String>,
    /// Literal substitutions for environment placeholders (`{PROJ}`, `{TMP}`, ...).
    pub literals: Vec<(String, String)>,
}

const ID_PREFIXES: &str =
    "ses|msg|per|que|prt|pty|evt|wrk|job|tool|prj|cpy|att|cred|int|tkt|ref|req";

impl Bindings {
    /// Substitute bound ids and literals into request text. Unbound ids get a deterministic
    /// well-formed stand-in so the request still decodes.
    pub fn fill(&self, text: &str) -> String {
        let mut out = self.fill_literals(text);
        let re = Regex::new(&format!(r"\{{({ID_PREFIXES})_(\d+)\}}")).unwrap();
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                let key = c[0].to_string();
                self.ids
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| format!("{}_unbound{:0>19}", &c[1], &c[2]))
            })
            .into_owned();
        out
    }

    fn fill_literals(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (placeholder, value) in &self.literals {
            out = out.replace(placeholder, value);
            let encoded = |s: &str| percent_encode(s);
            out = out.replace(&encoded(placeholder), &encoded(value));
        }
        out
    }

    pub fn fill_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.fill(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.fill_value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (self.fill(k), self.fill_value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Match `expected` (normalized fixture) against `actual`. Returns the JSON path of the
    /// first difference.
    pub fn matches(&mut self, expected: &Value, actual: &Value, path: &str) -> Result<(), String> {
        match (expected, actual) {
            (Value::String(e), Value::Number(n)) if e == "{TS}" || e == "{PID}" => {
                if n.is_i64() || n.is_u64() {
                    return Ok(());
                }
                Err(format!("{path}: expected integer for {e}, got {n}"))
            }
            (Value::String(e), Value::String(a))
                if e.starts_with("{CURSOR:") && e.ends_with('}') =>
            {
                let inner: Value = serde_json::from_str(&e[8..e.len() - 1])
                    .map_err(|err| format!("{path}: bad cursor fixture: {err}"))?;
                let decoded = decode_cursor(a)
                    .ok_or_else(|| format!("{path}: cursor {a:?} is not base64url JSON"))?;
                self.matches(&inner, &decoded, &format!("{path}<cursor>"))
            }
            (Value::String(e), Value::String(a)) => {
                self.text(e, a).map_err(|m| format!("{path}: {m}"))
            }
            (Value::Array(e), Value::Array(a)) => {
                if e.len() != a.len() {
                    return Err(format!("{path}: array length {} != {}", a.len(), e.len()));
                }
                for (i, (ev, av)) in e.iter().zip(a).enumerate() {
                    self.matches(ev, av, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            (Value::Object(e), Value::Object(a)) => {
                let ek: Vec<String> = e.keys().map(|k| self.fill_literals(k)).collect();
                let ak: Vec<&String> = a.keys().collect();
                let missing: Vec<&String> =
                    ek.iter().filter(|k| !a.contains_key(k.as_str())).collect();
                let extra: Vec<&&String> = ak.iter().filter(|k| !ek.contains(k)).collect();
                if !missing.is_empty() || !extra.is_empty() {
                    return Err(format!(
                        "{path}: missing keys {missing:?}, extra keys {extra:?}"
                    ));
                }
                for ((key, ev), filled) in e.iter().zip(&ek) {
                    self.matches(ev, &a[filled.as_str()], &format!("{path}.{key}"))?;
                }
                if ak
                    .iter()
                    .map(|k| k.as_str())
                    .ne(ek.iter().map(String::as_str))
                {
                    return Err(format!("{path}: key order {ak:?} != {ek:?}"));
                }
                Ok(())
            }
            (e, a) if e == a => Ok(()),
            (e, a) => Err(format!("{path}: expected {}, got {}", short(e), short(a))),
        }
    }

    /// Match a string with embedded placeholders.
    pub fn text(&mut self, expected: &str, actual: &str) -> Result<(), String> {
        let expected = self.fill_literals(expected);
        let token = Regex::new(&format!(
            r"\{{(?:({ID_PREFIXES})_\d+|SHA|UUID|ERR_REF|ISO|SLUG|TICKET|NAME|TS|PID|DEAD_PORT)\}}"
        ))
        .unwrap();
        let mut pattern = String::from("^");
        let mut names: Vec<String> = Vec::new();
        let mut last = 0;
        for m in token.find_iter(&expected) {
            pattern.push_str(&regex::escape(&expected[last..m.start()]));
            let tok = m.as_str();
            let piece = match tok {
                "{SHA}" => "[0-9a-f]{40}".to_string(),
                "{UUID}" => {
                    "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}".to_string()
                }
                "{ERR_REF}" => "err_[0-9a-f]{8}".to_string(),
                "{ISO}" => r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z".to_string(),
                "{TS}" | "{PID}" | "{DEAD_PORT}" => r"\d+".to_string(),
                "{SLUG}" | "{TICKET}" | "{NAME}" => ".+".to_string(),
                id => {
                    let prefix = &id[1..id.find('_').unwrap()];
                    match self.ids.get(id) {
                        Some(bound) => regex::escape(bound),
                        None => {
                            if !names.iter().any(|n| n == id) {
                                names.push(id.to_string());
                            }
                            format!("({prefix}_[0-9A-Za-z]{{26}})")
                        }
                    }
                }
            };
            pattern.push_str(&piece);
            last = m.end();
        }
        pattern.push_str(&regex::escape(&expected[last..]));
        pattern.push('$');
        let re = Regex::new(&pattern).map_err(|e| e.to_string())?;
        let Some(caps) = re.captures(actual) else {
            return Err(format!("{} != {}", short_str(actual), short_str(&expected)));
        };
        // Groups are numbered in order of first unbound occurrence; repeated unbound tokens in
        // one string each get a group, all must agree.
        let mut group = 1;
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for m in token.find_iter(&expected) {
            let tok = m.as_str();
            if tok.contains('_')
                && !tok.starts_with("{ERR")
                && !tok.starts_with("{DEAD")
                && !self.ids.contains_key(tok)
            {
                let value = caps
                    .get(group)
                    .map(|g| g.as_str().to_string())
                    .unwrap_or_default();
                group += 1;
                if let Some(prev) = seen.get(tok)
                    && prev != &value
                {
                    return Err(format!("{tok} bound to both {prev} and {value}"));
                }
                seen.insert(tok.to_string(), value);
            }
        }
        for (tok, value) in seen {
            if let Some((other, _)) = self.ids.iter().find(|(_, v)| **v == value) {
                return Err(format!(
                    "{value} already bound to {other}, fixture expects {tok}"
                ));
            }
            self.ids.insert(tok, value);
        }
        Ok(())
    }
}

fn decode_cursor(raw: &str) -> Option<Value> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn percent_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn short(v: &Value) -> String {
    short_str(&v.to_string())
}

fn short_str(s: &str) -> String {
    if s.chars().count() <= 160 {
        return format!("{s:?}");
    }
    format!("{:?}...", s.chars().take(160).collect::<String>())
}
