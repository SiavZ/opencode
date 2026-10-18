//! Purpose: legacy remote-workspace HTTP proxy (`httpapi/middleware/proxy.ts` `http` and
//! `packages/opencode/src/server/proxy-util.ts`): hop-by-hop and placement header stripping,
//! streamed success bodies, buffered and preserved 5xx bodies, empty 500 on transport failure.
//! Scope: per-request outbound call through a process-level reqwest client.
//! Uses:
//! `-- none
//! External:
//! |-- axum::body::Body
//! `-- reqwest::Client
//! Used by:
//! rust/crates/
//! `-- server/src/middleware/legacy_routing.rs
//! Invariants: request headers lose hop headers, `host`, `accept-encoding`,
//! `x-opencode-directory`, `x-opencode-workspace`, then extra target headers are set. Response
//! headers lose `content-encoding` and `content-length`. 5xx bodies keep their content type
//! (default `application/json`). Proxy URL drops the `workspace` and `directory` query params.
//! WebSocket upgrades are not relayed here (see status.md blocker).
//! Contract tests: server/tests/http_proxy.rs

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;

const HOP: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

/// `ProxyUtil.headers`: sanitized inbound headers plus `extra` overrides.
pub fn headers(input: &HeaderMap, extra: &[(String, String)]) -> HeaderMap {
    let mut out = input.clone();
    for key in HOP {
        out.remove(key);
    }
    out.remove(header::ACCEPT_ENCODING);
    out.remove("x-opencode-directory");
    out.remove("x-opencode-workspace");
    for (key, value) in extra {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(key.as_str()),
            HeaderValue::from_str(value),
        ) {
            out.insert(name, value);
        }
    }
    out
}

/// `workspaceProxyURL`: join target path with the request path, keep the query minus
/// `workspace` and `directory`.
pub fn proxy_url(target: &str, path: &str, query: Option<&str>) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(target).map_err(|e| e.to_string())?;
    let base = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{base}{path}"));
    let kept: Vec<(String, String)> = query
        .map(|q| {
            form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .filter(|(k, _)| k != "workspace" && k != "directory")
                .collect()
        })
        .unwrap_or_default();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    Ok(url)
}

/// Forward one request. Never fails: transport errors become an empty 500.
pub async fn http(
    client: &reqwest::Client,
    url: reqwest::Url,
    extra: &[(String, String)],
    method: Method,
    request_headers: &HeaderMap,
    body: Body,
) -> Response {
    let mut builder = client
        .request(method.clone(), url.clone())
        .headers(headers(request_headers, extra));
    if method != Method::GET && method != Method::HEAD {
        builder = builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    let upstream = match builder.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(%url, %error, "workspace proxy transport failure");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let status = upstream.status();
    let mut out_headers = upstream.headers().clone();
    out_headers.remove(header::CONTENT_ENCODING);
    out_headers.remove(header::CONTENT_LENGTH);
    if status.is_server_error() {
        let content_type = out_headers
            .remove(header::CONTENT_TYPE)
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));
        let text = upstream.text().await.unwrap_or_default();
        tracing::error!(%url, %method, status = status.as_u16(), body = %text.chars().take(2000).collect::<String>(), "workspace proxy upstream error");
        let mut response = Response::new(Body::from(text));
        *response.status_mut() = status;
        *response.headers_mut() = out_headers;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
        return response;
    }
    // Upstream stream errors end the body early, like `Stream.catchCause(() => Stream.empty)`.
    let stream = upstream
        .bytes_stream()
        .map_err(std::io::Error::other)
        .or_else(|_| async { Ok::<_, std::io::Error>(bytes::Bytes::new()) });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    response
}
