//! Purpose: redaction of request/response diagnostics (packages/llm/src/route/executor.ts
//! `redactHeaders`, `redactUrl`, `secretValues`, `redactBody`, `responseBody`). Header, URL
//! query and body field redaction plus literal removal of secrets echoed by the provider.
//! Scope: pure string transforms, no I/O.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/auth.rs
//! External:
//! |-- regex::Regex
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/executor.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/src/transport/websocket.rs
//! Invariants: one sensitive-name source drives headers, query keys and body fields. Values
//! shorter than 4 characters are never treated as literal secrets. Bodies are truncated to
//! `BODY_LIMIT` UTF-16 code units (JS `String.length`) after redaction, never before.
//! Contract tests: provider-adapters/tests/transport_executor.rs, provider-adapters/src/transport/redact.rs (unit tests)

use std::sync::LazyLock;

use regex::Regex;

use crate::auth::Headers;

pub const REDACTED: &str = "<redacted>";
/// Maximum diagnostic body length in UTF-16 code units.
pub const BODY_LIMIT: usize = 16_384;

const SENSITIVE_NAME_SOURCE: &str = "authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|id[-_]?token|token|secret|credential|signature|x-amz-signature";

static SENSITIVE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i){SENSITIVE_NAME_SOURCE}")).expect("static regex"));
static SHORT_QUERY_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)^(key|sig)$").expect("static regex"));
static REDACT_JSON_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)("(?:(?:{SENSITIVE_NAME_SOURCE}|key))"\s*:\s*)"[^"]*""#
    ))
    .expect("static regex")
});
static REDACT_QUERY_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)((?:(?:{SENSITIVE_NAME_SOURCE}|key))=)[^&\s"]+"#
    ))
    .expect("static regex")
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Bearer\s+(.+)$").expect("static regex"));

/// One entry of `Headers.CurrentRedactedNames`: exact (case-insensitive) name or a pattern.
#[derive(Clone, Debug)]
pub enum RedactedName {
    Exact(String),
    Pattern(Regex),
}

impl RedactedName {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Exact(exact) => exact.eq_ignore_ascii_case(name),
            Self::Pattern(re) => re.is_match(name),
        }
    }
}

/// Effect `Headers.CurrentRedactedNames` default value.
pub fn default_redacted_names() -> Vec<RedactedName> {
    ["authorization", "cookie", "set-cookie", "x-api-key"]
        .into_iter()
        .map(|n| RedactedName::Exact(n.to_owned()))
        .collect()
}

pub fn is_sensitive_header_name(name: &str) -> bool {
    SENSITIVE_NAME.is_match(name)
}

pub fn is_sensitive_query_name(name: &str) -> bool {
    is_sensitive_header_name(name) || SHORT_QUERY_NAME.is_match(name)
}

/// `redactHeaders`: lowercase names, redact configured names plus any sensitive-looking name.
pub fn redact_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    redacted_names: &[RedactedName],
) -> Headers {
    headers
        .into_iter()
        .map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            let hidden =
                redacted_names.iter().any(|r| r.matches(&name)) || is_sensitive_header_name(&name);
            let value = if hidden { REDACTED } else { value };
            (name, value.to_owned())
        })
        .collect()
}

/// `redactUrl`: unparsable URLs become `<redacted>`; sensitive query values are replaced
/// with `URLSearchParams.set` semantics (first occurrence replaced, later duplicates removed,
/// query re-serialized as `application/x-www-form-urlencoded`).
pub fn redact_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return REDACTED.to_owned();
    };
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if !pairs.iter().any(|(k, _)| is_sensitive_query_name(k)) {
        return url.to_string();
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut out = url.query_pairs_mut();
    out.clear();
    for (key, val) in &pairs {
        if !is_sensitive_query_name(key) {
            out.append_pair(key, val);
            continue;
        }
        if seen.contains(&key.as_str()) {
            continue;
        }
        seen.push(key);
        out.append_pair(key, REDACTED);
    }
    drop(out);
    url.to_string()
}

/// JS `encodeURIComponent`.
pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `secretValues`: literal secrets sent in sensitive headers and query parameters.
pub fn secret_values(url: &str, headers: &Headers) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut add = |value: &str| {
        if value.chars().count() < 4 {
            return;
        }
        for candidate in [value.to_owned(), encode_uri_component(value)] {
            if !values.contains(&candidate) {
                values.push(candidate);
            }
        }
    };
    for (name, value) in headers {
        if !is_sensitive_header_name(name) {
            continue;
        }
        add(value);
        if let Some(bearer) = BEARER.captures(value).and_then(|c| c.get(1)) {
            add(bearer.as_str());
        }
    }
    if let Ok(parsed) = url::Url::parse(url) {
        for (key, value) in parsed.query_pairs() {
            if is_sensitive_query_name(&key) {
                add(&value);
            }
        }
    }
    values
}

/// `redactBody`: structural pass (`"name": "value"`, `name=value`) then literal secret pass.
pub fn redact_body(body: &str, secrets: &[String]) -> String {
    let structural = REDACT_JSON_FIELD.replace_all(body, format!("${{1}}\"{REDACTED}\""));
    let mut text = REDACT_QUERY_FIELD
        .replace_all(&structural, format!("${{1}}{REDACTED}"))
        .into_owned();
    for secret in secrets {
        if text.contains(secret.as_str()) {
            text = text.replace(secret.as_str(), REDACTED);
        }
    }
    text
}

/// JS `String.length` (UTF-16 code units).
pub fn js_length(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `responseBody`: redacted body plus `bodyTruncated` when it exceeds `BODY_LIMIT`.
pub fn response_body(body: &str, secrets: &[String]) -> (String, Option<bool>) {
    let redacted = redact_body(body, secrets);
    if js_length(&redacted) <= BODY_LIMIT {
        return (redacted, None);
    }
    let mut units = 0;
    let end = redacted
        .char_indices()
        .find(|(_, ch)| {
            units += ch.len_utf16();
            units > BODY_LIMIT
        })
        .map_or(redacted.len(), |(idx, _)| idx);
    (redacted[..end].to_owned(), Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_query_redaction_uses_search_params_set() {
        assert_eq!(
            redact_url("https://p.test/v1/chat?api_key=secret&key=secret&debug=1"),
            "https://p.test/v1/chat?api_key=%3Credacted%3E&key=%3Credacted%3E&debug=1"
        );
        assert_eq!(
            redact_url("https://p.test/x?sig=a&sig=b&x=1"),
            "https://p.test/x?sig=%3Credacted%3E&x=1"
        );
        assert_eq!(redact_url("https://p.test/x?a=1"), "https://p.test/x?a=1");
        assert_eq!(redact_url("not a url"), REDACTED);
    }

    #[test]
    fn body_redaction_structural_and_literal() {
        let headers: Headers = [("authorization".into(), "Bearer header-secret-456".into())]
            .into_iter()
            .collect();
        let secrets = secret_values("https://p.test/?api_key=query-secret-123&k=v", &headers);
        assert!(secrets.contains(&"header-secret-456".to_owned()));
        assert!(secrets.contains(&"Bearer%20header-secret-456".to_owned()));
        let body = r#"{"key":"body-secret","detail":"api_key=query-secret","x":"query-secret-123 header-secret-456"}"#;
        let out = redact_body(body, &secrets);
        assert_eq!(
            out,
            r#"{"key":"<redacted>","detail":"api_key=<redacted>","x":"<redacted> <redacted>"}"#
        );
        assert!(secret_values("https://p.test/?key=abc", &Headers::new()).is_empty());
    }

    #[test]
    fn truncation_counts_utf16_units() {
        let (body, truncated) = response_body(&"x".repeat(20_000), &[]);
        assert_eq!((body.len(), truncated), (BODY_LIMIT, Some(true)));
        let (body, truncated) = response_body(&"\u{1F600}".repeat(BODY_LIMIT), &[]);
        assert_eq!((js_length(&body), truncated), (BODY_LIMIT, Some(true)));
        assert_eq!(response_body("ok", &[]), ("ok".to_owned(), None));
    }

    #[test]
    fn header_redaction() {
        let out = redact_headers(
            [
                ("X-Safe", "v"),
                ("X-Api-Key", "s"),
                ("Cookie", "c"),
                ("x-goog-api-key", "g"),
            ],
            &default_redacted_names(),
        );
        let pairs: Vec<(&str, &str)> = out.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("x-safe", "v"),
                ("x-api-key", REDACTED),
                ("cookie", REDACTED),
                ("x-goog-api-key", REDACTED)
            ]
        );
    }
}
