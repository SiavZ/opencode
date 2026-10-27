//! Purpose: reproduce language.ts request locale selection for native gateway errors.
//! Scope: explicit header, route/docs path, percent-encoded cookie and Accept-Language precedence.
//! Uses:
//! `-- none
//! External:
//! `-- http::HeaderMap
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/locale.rs
//! Invariants: explicit locales are case-sensitive; language tags use baseline prefix matching.
//! Contract tests: hosted/src/console/zen/locale/tests.rs

use http::HeaderMap;

const LOCALES: [&str; 18] = [
    "en", "zh", "zht", "ko", "de", "es", "fr", "it", "da", "ja", "pl", "ru", "uk", "ar", "no",
    "br", "th", "tr",
];

pub(super) fn select(headers: &HeaderMap, path: &str) -> &'static str {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(locale) = header("x-opencode-locale").and_then(exact) {
        return locale;
    }
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let mut segments = path.split('/');
    segments.next();
    let first = segments.next().unwrap_or_default();
    if let Some(locale) = exact(first) {
        return locale;
    }
    if first == "docs" {
        let language = segments.next().unwrap_or_default().to_lowercase();
        let locale = match language.as_str() {
            "root" | "en" => Some("en"),
            "zh-cn" => Some("zh"),
            "zh-tw" => Some("zht"),
            "nb" => Some("no"),
            "pt-br" => Some("br"),
            "ar" | "da" | "de" | "es" | "fr" | "it" | "ja" | "ko" | "pl" | "ru" | "th" | "tr"
            | "uk" => exact(&language),
            _ => None,
        };
        if let Some(locale) = locale {
            return locale;
        }
    }
    if let Some(cookie) = header("cookie").and_then(|cookie| {
        cookie
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("oc_locale="))
    }) && let Some(locale) = decode(cookie).as_deref().and_then(exact)
    {
        return locale;
    }
    let mut languages = header("accept-language")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut pieces = part.split(';').map(str::trim);
            let language = pieces.next().unwrap_or_default();
            let quality = pieces
                .find_map(|part| part.strip_prefix("q="))
                .filter(|value| !value.is_empty())
                .map(parse_float)
                .unwrap_or(1.0);
            (language, quality)
        })
        .collect::<Vec<_>>();
    // JS treats a NaN comparator result as equal, and sort is stable.
    languages.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    languages
        .iter()
        .filter(|(language, _)| !language.is_empty() && *language != "*")
        .find_map(|(language, _)| language_match(language))
        .unwrap_or("en")
}

fn exact(value: &str) -> Option<&'static str> {
    LOCALES.iter().copied().find(|locale| *locale == value)
}
fn language_match(language: &str) -> Option<&'static str> {
    let value = language.trim().to_lowercase();
    if value.starts_with("zh") {
        return Some(
            if ["hant", "-tw", "-hk", "-mo"]
                .iter()
                .any(|part| value.contains(part))
            {
                "zht"
            } else {
                "zh"
            },
        );
    }
    for language in [
        "ko", "de", "es", "fr", "it", "da", "ja", "pl", "ru", "uk", "ar", "tr", "th",
    ] {
        if value.starts_with(language) {
            return exact(language);
        }
    }
    if value.starts_with("pt") {
        return Some("br");
    }
    if ["no", "nb", "nn"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
    {
        return Some("no");
    }
    if value.starts_with("en") {
        return Some("en");
    }
    None
}
fn decode(raw: &str) -> Option<String> {
    let mut decoded = Vec::new();
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let high = char::from(bytes.next()?).to_digit(16)?;
        let low = char::from(bytes.next()?).to_digit(16)?;
        decoded.push((high * 16 + low) as u8);
    }
    String::from_utf8(decoded).ok()
}
fn parse_float(input: &str) -> f64 {
    let input = input.trim_start();
    if input.starts_with("Infinity") || input.starts_with("+Infinity") {
        return f64::INFINITY;
    }
    if input.starts_with("-Infinity") {
        return f64::NEG_INFINITY;
    }
    input
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .rev()
        .find_map(|end| input[..end].parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}
