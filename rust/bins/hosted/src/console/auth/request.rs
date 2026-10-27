//! Purpose: trusted console origin, local redirects and baseline locale selection.
//! Scope: HTTP request validation and URL construction.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/auth.rs
//! External:
//! |-- axum::http::HeaderMap
//! `-- url::Url
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/auth.rs
//! Invariants: configured origin replaces untrusted proxy/Host values; redirects remain local.
//! Contract tests: hosted/src/console/auth/tests.rs

#![allow(clippy::result_large_err)]

use super::{Config, ConsoleState, error};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;

pub(super) fn parse_origin(value: &str) -> Result<url::Url, ()> {
    let url = url::Url::parse(value).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    Ok(url)
}

pub(super) fn url(config: &Config, uri: &Uri, _headers: &HeaderMap) -> Result<url::Url, Response> {
    let bad = || error(StatusCode::BAD_REQUEST, "Invalid request origin");
    let origin = config.public_origin.as_deref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Console public origin is not configured",
        )
    })?;
    let mut base = parse_origin(origin).map_err(|_| bad())?;
    if uri.scheme().is_some() && uri.authority().is_some() {
        let request_origin = url::Url::parse(&uri.to_string()).map_err(|_| bad())?;
        if request_origin.origin() != base.origin() {
            return Err(bad());
        }
    }
    base.set_path(uri.path());
    base.set_query(uri.query());
    Ok(base)
}

/// New JSON action routes reject cross-origin browser requests. Non-browser clients may omit Origin.
pub fn require_same_origin(state: &ConsoleState, headers: &HeaderMap) -> Result<(), Response> {
    let denied = || {
        error(
            StatusCode::FORBIDDEN,
            "Cross-origin console action is not allowed",
        )
    };
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return Err(denied());
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = parse_origin(origin.to_str().map_err(|_| denied())?).map_err(|_| denied())?;
    let expected = url(&state.config.auth, &Uri::from_static("/"), headers)?;
    if origin.origin() != expected.origin() {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn require_local_path(path: &str) -> Result<(), Response> {
    if path.is_empty() {
        return Ok(());
    }
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Invalid local continuation path",
        ));
    }
    Ok(())
}

const LOCALES: [&str; 18] = [
    "en", "zh", "zht", "ko", "de", "es", "fr", "it", "da", "ja", "pl", "ru", "uk", "ar", "no",
    "br", "th", "tr",
];

fn locale(headers: &HeaderMap) -> &str {
    if let Some(value) = headers
        .get("x-opencode-locale")
        .and_then(|v| v.to_str().ok())
        .filter(|v| LOCALES.contains(v))
    {
        return value;
    }
    if let Some(value) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|v| v.strip_prefix("oc_locale="))
        })
        .filter(|v| LOCALES.contains(v))
    {
        return value;
    }
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',')
                .filter_map(|item| {
                    let mut parts = item.trim().split(';');
                    let lang = parts.next()?.split('-').next()?;
                    let quality = parts
                        .find_map(|v| v.trim().strip_prefix("q="))
                        .and_then(|q| q.parse::<f32>().ok())
                        .unwrap_or(1.0);
                    let lang = match lang {
                        "pt" => "br",
                        "nb" | "nn" => "no",
                        other => other,
                    };
                    LOCALES.contains(&lang).then_some((lang, quality))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|v| v.0)
        })
        .unwrap_or("en")
}

pub(super) fn localized_path(headers: &HeaderMap, path: &str) -> String {
    let path = LOCALES
        .iter()
        .find_map(|lang| {
            path.strip_prefix(&format!("/{lang}/"))
                .map(|rest| format!("/{rest}"))
        })
        .unwrap_or_else(|| path.to_string());
    if path.starts_with("/auth") || path.starts_with("/workspace") || locale(headers) == "en" {
        return path;
    }
    if path == "/" {
        return format!("/{}", locale(headers));
    }
    format!("/{}{path}", locale(headers))
}

pub(super) fn missing_code_message(headers: &HeaderMap) -> &'static str {
    match locale(headers) {
        "zh" => "未找到授权码。",
        "zht" => "找不到授權碼。",
        "ko" => "인증 코드를 찾을 수 없습니다.",
        "de" => "Kein Autorisierungscode gefunden.",
        "es" => "No se encontró código de autorización.",
        "fr" => "Aucun code d'autorisation trouvé.",
        "it" => "Nessun codice di autorizzazione trovato.",
        "da" => "Ingen autorisationskode fundet.",
        "ja" => "認証コードが見つかりません。",
        "pl" => "Nie znaleziono kodu autoryzacji.",
        "ru" => "Код авторизации не найден.",
        "uk" => "Не знайдено код авторизації.",
        "ar" => "لم يتم العثور على رمز التفويض.",
        "no" => "Ingen autorisasjonskode funnet.",
        "br" => "Nenhum código de autorização encontrado.",
        "th" => "ไม่พบ authorization code",
        "tr" => "Yetkilendirme kodu bulunamadı.",
        _ => "No authorization code found.",
    }
}
