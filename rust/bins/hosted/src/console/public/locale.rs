//! Purpose: reproduce console locale negotiation, docs paths and locale cookies.
//! Scope: pure auxiliary route localization.
//! Uses:
//! `-- none
//! External:
//! `-- axum::http::HeaderMap
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: explicit header then URL locale then cookie then weighted language.
//! Contract tests: public/tests.rs

use axum::http::HeaderMap;
pub const LOCALES: [&str; 18] = [
    "en", "zh", "zht", "ko", "de", "es", "fr", "it", "da", "ja", "pl", "ru", "uk", "ar", "no",
    "br", "th", "tr",
];
pub fn parse(value: &str) -> Option<&'static str> {
    LOCALES.into_iter().find(|locale| *locale == value)
}
pub fn explicit(headers: &HeaderMap) -> Option<&'static str> {
    headers
        .get("x-opencode-locale")
        .and_then(|value| value.to_str().ok())
        .and_then(parse)
        .or_else(|| {
            headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value
                        .split(';')
                        .find_map(|part| part.trim().strip_prefix("oc_locale=").and_then(parse))
                })
        })
}
pub fn select(headers: &HeaderMap, path: &str) -> &'static str {
    if let Some(value) = headers
        .get("x-opencode-locale")
        .and_then(|value| value.to_str().ok())
        .and_then(parse)
    {
        return value;
    }
    if let Some(value) = path.split('/').nth(1).and_then(parse) {
        return value;
    }
    if path.starts_with("/docs/")
        && let Some(value) = path.split('/').nth(2).and_then(|value| match value {
            "zh-cn" => Some("zh"),
            "zh-tw" => Some("zht"),
            "nb" => Some("no"),
            "pt-br" => Some("br"),
            "root" => Some("en"),
            other => parse(other),
        })
    {
        return value;
    }
    if let Some(value) = explicit(headers) {
        return value;
    }
    let mut values = headers
        .get("accept-language")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(|item| {
            let mut parts = item.trim().split(';');
            let language = parts.next().unwrap_or("").to_ascii_lowercase();
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .map_or(1.0, |value| value.parse::<f32>().unwrap_or(f32::NAN));
            (language, quality)
        })
        .collect::<Vec<_>>();
    values.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    values
        .into_iter()
        .find_map(|(value, _)| language(&value))
        .unwrap_or("en")
}
fn language(value: &str) -> Option<&'static str> {
    if value.starts_with("zh") {
        return Some(
            if value.contains("hant")
                || value.contains("-tw")
                || value.contains("-hk")
                || value.contains("-mo")
            {
                "zht"
            } else {
                "zh"
            },
        );
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
    LOCALES
        .into_iter()
        .filter(|value| !["zh", "zht", "no", "br"].contains(value))
        .find(|prefix| value.starts_with(prefix))
}
pub fn tag(locale: &str) -> &str {
    match locale {
        "zh" => "zh-Hans",
        "zht" => "zh-Hant",
        "br" => "pt-BR",
        other => other,
    }
}
pub fn cookie(locale: &str) -> String {
    format!("oc_locale={locale}; Path=/; Max-Age=31536000; SameSite=Lax")
}
pub fn docs(locale: &str, path: &str) -> String {
    if locale == "en" {
        if path == "/docs/en" {
            return "/docs".into();
        }
        if let Some(rest) = path.strip_prefix("/docs/en/") {
            return format!("/docs/{rest}");
        }
        return path.into();
    }
    let value = match locale {
        "zh" => "zh-cn",
        "zht" => "zh-tw",
        "no" => "nb",
        "br" => "pt-br",
        other => other,
    };
    if path == "/docs" {
        return format!("/docs/{value}");
    }
    if path == "/docs/" {
        return format!("/docs/{value}/");
    }
    let Some(rest) = path.strip_prefix("/docs/") else {
        return path.into();
    };
    let head = rest.split('/').next().unwrap_or("");
    if [
        "ar", "bs", "da", "de", "es", "fr", "it", "ja", "ko", "nb", "pl", "pt-br", "ru", "th",
        "tr", "uk", "zh-cn", "zh-tw",
    ]
    .contains(&head)
        || head.starts_with('_')
        || head.contains('.')
    {
        return path.into();
    }
    format!("/docs/{value}/{rest}")
}
pub fn required(locale: &str) -> &'static str {
    match locale {
        "fr" => "Tous les champs sont requis.",
        "br" => "Todos os campos são obrigatórios.",
        "da" => "Alle felter er påkrævet.",
        "es" => "Todos los campos son obligatorios.",
        "de" => "Alle Felder sind erforderlich.",
        "it" => "Tutti i campi sono obbligatori.",
        "ko" => "모든 필드는 필수 항목입니다.",
        "zht" => "所有欄位均為必填。",
        "no" => "Alle felt er obligatoriske.",
        "ja" => "すべての項目は必須です。",
        "ar" => "جميع الحقول مطلوبة.",
        "zh" => "所有字段均为必填项。",
        "ru" => "Все поля обязательны.",
        "th" => "จำเป็นต้องกรอกทุกช่อง",
        "pl" => "Wszystkie pola są wymagane.",
        "tr" => "Tüm alanlar gereklidir.",
        "uk" => "Усі поля обов'язкові.",
        _ => "All fields are required.",
    }
}
