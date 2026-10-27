//! Purpose: reproduce console data proxy locale negotiation, cache isolation and HTML rewrites.
//! Scope: fixed stats upstream with process-local response cache.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/public.rs
//! `-- hosted/src/console/public/locale.rs
//! External:
//! |-- axum::{body::Body, extract::Request}
//! `-- std::sync::{Arc, Mutex}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: Markdown/assets/API requests bypass HTML cache; redirect locations stay public.
//! Contract tests: public/data_tests.rs

use super::{ConsoleState, Error, failure, locale, origin, redirect};
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Cache(Arc<Mutex<HashMap<String, Cached>>>);
#[derive(Clone, Debug)]
struct Cached {
    expires: Instant,
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}
impl Cache {
    pub(super) fn get(&self, key: &str) -> Option<Response> {
        let mut cache = self.0.lock().expect("public cache lock");
        cache.retain(|_, item| item.expires > Instant::now());
        cache.get(key).map(|item| {
            let mut response = (item.status, Body::from(item.body.clone())).into_response();
            *response.headers_mut() = item.headers.clone();
            response
        })
    }
    pub(super) fn put(
        &self,
        key: String,
        status: StatusCode,
        headers: HeaderMap,
        body: Vec<u8>,
        ttl: u64,
    ) {
        let mut cache = self.0.lock().expect("public cache lock");
        cache.retain(|_, item| item.expires > Instant::now());
        // The baseline edge cache is bounded by the platform; bound native storage as well.
        if cache.len() >= 256
            && let Some(key) = cache.keys().next().cloned()
        {
            cache.remove(&key);
        }
        if body.len() <= 8 * 1024 * 1024 && ttl > 0 {
            cache.insert(
                key,
                Cached {
                    expires: Instant::now() + Duration::from_secs(ttl),
                    status,
                    headers,
                    body,
                },
            );
        }
    }
}

fn bypass(path: &str) -> bool {
    ["/data/_build/", "/data/api/", "/data/_server"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || ["/data/banner.jpg", "/data/banner.png", "/data/sitemap.xml"].contains(&path)
        || ["/llms.txt", ".md", ".json"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
}

fn html(headers: &HeaderMap) -> bool {
    let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let weights = accept
        .split(',')
        .map(|part| {
            let mut fields = part
                .split(';')
                .map(|value| value.trim().to_ascii_lowercase());
            let mime = fields.next().unwrap_or_default();
            let quality = fields
                .find_map(|field| field.strip_prefix("q=").map(str::to_owned))
                .map_or(1.0, |value| value.parse::<f64>().unwrap_or(f64::NAN));
            (mime, quality)
        })
        .collect::<HashMap<_, _>>();
    let markdown = weights
        .get("text/markdown")
        .copied()
        .unwrap_or(0.0)
        .max(weights.get("text/x-markdown").copied().unwrap_or(0.0));
    if markdown > 0.0 && markdown >= weights.get("text/html").copied().unwrap_or(0.0) {
        return false;
    }
    accept.contains("text/html") || accept.contains("*/*")
}

fn vary(headers: &mut HeaderMap) {
    let mut values = headers
        .get(header::VARY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for value in ["Accept", "Accept-Language", "Cookie", "x-opencode-locale"] {
        if !values.iter().any(|item| item.eq_ignore_ascii_case(value)) {
            values.push(value.into());
        }
    }
    headers.insert(header::VARY, values.join(", ").parse().unwrap());
}

pub async fn proxy(State(state): State<ConsoleState>, request: Request) -> Result<Response, Error> {
    let selected = locale::select(request.headers(), request.uri().path());
    let accepts_html = html(request.headers());
    let bypass = bypass(request.uri().path());
    let mut public = origin(&state)?;
    public.set_path(request.uri().path());
    public.set_query(request.uri().query());
    if selected != "en"
        && !request.headers().contains_key("x-opencode-locale")
        && matches!(*request.method(), Method::GET | Method::HEAD)
        && accepts_html
        && !bypass
    {
        public.set_path(&format!("/{selected}{}", request.uri().path()));
        let mut response = redirect(public.as_str(), StatusCode::PERMANENT_REDIRECT);
        response.headers_mut().append(
            header::SET_COOKIE,
            locale::cookie(selected).parse().unwrap(),
        );
        vary(response.headers_mut());
        return Ok(response);
    }
    let cache_key = if request.method() == Method::GET && accepts_html && !bypass {
        let add = public
            .query_pairs()
            .find(|(key, _)| key == "add")
            .map(|(_, value)| value.into_owned());
        let mut key = public.clone();
        key.set_query(None);
        if let Some(add) = add.filter(|value| !value.is_empty()) {
            key.query_pairs_mut().append_pair("add", &add);
        }
        key.query_pairs_mut()
            .append_pair("__opencode_stats_locale", selected);
        Some(format!("stats:{key}"))
    } else {
        None
    };
    if let Some(cached) = cache_key
        .as_deref()
        .and_then(|key| state.config.public.cache.get(key))
    {
        let mut cached = cached;
        cached
            .headers_mut()
            .insert("x-opencode-stats-cache", "HIT".parse().unwrap());
        return Ok(cached);
    }
    let base = state.config.public.data_origin.as_deref().unwrap_or(
        if state.config.stage == "production" {
            "https://stats.opencode.ai"
        } else {
            "https://stats.dev.opencode.ai"
        },
    );
    let mut upstream = url::Url::parse(base)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "Invalid stats origin"))?;
    let path = request.uri().path();
    upstream.set_path(
        if path.starts_with("/data/_build/")
            || ["/data/banner.jpg", "/data/banner.png", "/data/sitemap.xml"].contains(&path)
        {
            &path[5..]
        } else {
            path
        },
    );
    upstream.set_query(request.uri().query());
    let mut headers = request.headers().clone();
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);
    headers.remove(header::CONNECTION);
    headers.insert("x-opencode-locale", selected.parse().unwrap());
    headers.insert(
        header::ACCEPT_LANGUAGE,
        locale::tag(selected).parse().unwrap(),
    );
    let method = request.method().clone();
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Invalid stats request body"))?;
    let response = state
        .config
        .public
        .manual_http
        .request(method, upstream.clone())
        .headers(headers)
        .body(bytes)
        .send()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Stats upstream request failed"))?;
    let status = response.status();
    let mut headers = response.headers().clone();
    let bytes = response
        .bytes()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Stats upstream response failed"))?;
    if let Some(location) = headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(location) = upstream.join(location)
            && location.origin() == upstream.origin()
        {
            let value = format!(
                "{}{}{}",
                location.path(),
                location
                    .query()
                    .map(|value| format!("?{value}"))
                    .unwrap_or_default(),
                location
                    .fragment()
                    .map(|value| format!("#{value}"))
                    .unwrap_or_default()
            );
            headers.insert(
                header::LOCATION,
                value
                    .parse()
                    .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Invalid stats redirect"))?,
            );
        }
        let mut response = (status, Body::from(bytes)).into_response();
        *response.headers_mut() = headers;
        return Ok(response);
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"))
    {
        let mut response = (status, Body::from(bytes)).into_response();
        *response.headers_mut() = headers;
        return Ok(response);
    }
    for name in [
        header::CONTENT_ENCODING,
        header::CONTENT_LENGTH,
        header::ETAG,
        header::SET_COOKIE,
    ] {
        headers.remove(name);
    }
    vary(&mut headers);
    let body = String::from_utf8_lossy(&bytes)
        .replace("\"/_build/", "\"/data/_build/")
        .replace("'/_build/", "'/data/_build/")
        .into_bytes();
    if let Some(key) = cache_key.filter(|_| status.is_success()) {
        let control = headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let directives = control.split(',').map(str::trim).collect::<Vec<_>>();
        let ttl = if directives
            .iter()
            .any(|value| ["no-store", "private", "no-cache"].contains(value))
        {
            0
        } else {
            directives
                .iter()
                .find_map(|value| {
                    value
                        .strip_prefix("s-maxage=")
                        .and_then(|value| value.parse().ok())
                })
                .or_else(|| {
                    directives.iter().find_map(|value| {
                        value
                            .strip_prefix("max-age=")
                            .and_then(|value| value.parse().ok())
                    })
                })
                .unwrap_or(7200)
        };
        state
            .config
            .public
            .cache
            .put(key, status, headers.clone(), body.clone(), ttl);
        headers.insert("x-opencode-stats-cache", "MISS".parse().unwrap());
    }
    let mut response = (status, Body::from(body)).into_response();
    *response.headers_mut() = headers;
    Ok(response)
}
