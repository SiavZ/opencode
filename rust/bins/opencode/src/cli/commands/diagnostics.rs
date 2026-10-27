//! Purpose: show resolved configuration without exposing credential material.
//! Scope: debug config output, never modifies provider configuration.
//! Uses:
//! rust/crates/
//! `-- runtime/src/lib.rs
//! External:
//! |-- regex::Regex
//! |-- serde_json::Value
//! `-- url::Url
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/mod.rs
//! Invariants: headers, secrets and credential-bearing URLs never appear in debug JSON.
//! Contract tests: opencode/tests/admin/mod.rs

use serde_json::Value;
use std::sync::LazyLock;

static SECRET: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(?:api.?key|secret|password|token$|authorization$|cookie$|credential|private.?key)",
    )
    .expect("constant secret-name pattern")
});

pub fn redact(value: &mut Value, headers: bool) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|item| redact(item, headers)),
        Value::Object(items) => {
            for (key, value) in items {
                if let Some(text) = value.as_str() {
                    if headers || SECRET.is_match(key) {
                        *value = Value::String("***".into());
                        continue;
                    }
                    if text
                        .get(..7)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
                        || text
                            .get(..8)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
                    {
                        let sensitive = url::Url::parse(text).map_or(true, |url| {
                            !url.username().is_empty()
                                || url.password().is_some()
                                || url.query_pairs().any(|(key, _)| SECRET.is_match(&key))
                        });
                        if sensitive {
                            *value = Value::String("***".into());
                            continue;
                        }
                    }
                }
                redact(value, headers || key.eq_ignore_ascii_case("headers"));
            }
        }
        _ => {}
    }
}
