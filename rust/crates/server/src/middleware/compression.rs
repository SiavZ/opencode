//! Purpose: full-host response compression (`httpapi/middleware/compression.ts`): gzip or
//! deflate for buffered compressible bodies of at least 1024 bytes.
//! Scope: per-response.
//! Uses:
//! `-- none
//! External:
//! |-- axum::middleware::Next
//! `-- flate2::write::{DeflateEncoder, GzEncoder}
//! Used by:
//! rust/crates/
//! `-- server/src/app.rs
//! Invariants: skips HEAD, responses with content-encoding or transfer-encoding, streaming
//! bodies (no exact size hint), bodies under 1024 bytes, `cache-control: no-transform`,
//! `/event`, `/global/event`, `POST /session/:id/message|prompt_async`, and content types
//! outside the baseline compressible regex. Encoding preference: gzip, then deflate (substring
//! match on `accept-encoding`, like the baseline). `content-length` is rewritten.
//! Contract tests: server/tests/http_routes.rs

use std::io::Write;

use axum::body::HttpBody;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::Response;
use flate2::Compression;
use flate2::write::{DeflateEncoder, GzEncoder};

const THRESHOLD_BYTES: usize = 1024;

pub async fn layer(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let accept = request
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(str::to_ascii_lowercase);
    let response = next.run(request).await;
    if method == Method::HEAD
        || response.headers().contains_key(header::CONTENT_ENCODING)
        || response.headers().contains_key(header::TRANSFER_ENCODING)
    {
        return response;
    }
    // Only buffered bodies (exact size known) are eligible, like `body._tag === "Uint8Array"`.
    let Some(size) = response.body().size_hint().exact() else {
        return response;
    };
    if (size as usize) < THRESHOLD_BYTES {
        return response;
    }
    let no_transform = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|d| d.trim().eq_ignore_ascii_case("no-transform"))
        });
    if no_transform || streaming_path(&method, &path) {
        return response;
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !compressible(content_type) {
        return response;
    }
    let encoding = match accept {
        Some(a) if a.contains("gzip") => "gzip",
        Some(a) if a.contains("deflate") => "deflate",
        _ => return response,
    };
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, usize::MAX).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let compressed = match encoding {
        "gzip" => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&bytes).and_then(|_| encoder.finish())
        }
        _ => {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&bytes).and_then(|_| encoder.finish())
        }
    };
    let Ok(compressed) = compressed else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    parts
        .headers
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(compressed))
}

fn streaming_path(method: &Method, path: &str) -> bool {
    if path == "/event" || path == "/global/event" {
        return true;
    }
    if method != Method::POST {
        return false;
    }
    let Some(rest) = path.strip_prefix("/session/") else {
        return false;
    };
    let mut parts = rest.splitn(2, '/');
    let id = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("");
    !id.is_empty() && (tail == "message" || tail == "prompt_async")
}

/// The baseline `COMPRESSIBLE_CONTENT_TYPE_REGEX`.
pub fn compressible(content_type: &str) -> bool {
    let media = content_type
        .trim_start()
        .split([';', ' ', '\t'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let Some((kind, sub)) = media.split_once('/') else {
        return false;
    };
    if sub.is_empty() {
        return false;
    }
    match kind {
        "text" => sub != "event-stream",
        "application" => {
            matches!(
                sub,
                "javascript"
                    | "json"
                    | "xml"
                    | "xml-dtd"
                    | "ecmascript"
                    | "dart"
                    | "postscript"
                    | "rtf"
                    | "tar"
                    | "toml"
                    | "vnd.dart"
                    | "vnd.ms-fontobject"
                    | "vnd.ms-opentype"
                    | "wasm"
                    | "x-httpd-php"
                    | "x-javascript"
                    | "x-ns-proxy-autoconfig"
                    | "x-sh"
                    | "x-tar"
                    | "x-www-form-urlencoded"
            ) || suffixed(sub)
        }
        "font" => matches!(sub, "otf" | "ttf") || suffixed(sub),
        "image" => {
            matches!(
                sub,
                "bmp"
                    | "vnd.adobe.photoshop"
                    | "vnd.microsoft.icon"
                    | "vnd.ms-dds"
                    | "x-icon"
                    | "x-ms-bmp"
            ) || suffixed(sub)
        }
        "message" => sub == "rfc822" || suffixed(sub),
        "model" => sub == "gltf-binary" || suffixed(sub),
        "x-shader" => matches!(sub, "x-fragment" | "x-vertex") || suffixed(sub),
        _ => suffixed(sub),
    }
}

/// `[^;\s]+?\+(?:json|text|xml|yaml)`.
fn suffixed(sub: &str) -> bool {
    sub.rsplit_once('+')
        .is_some_and(|(_, s)| matches!(s, "json" | "text" | "xml" | "yaml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressible_matches_baseline_regex() {
        assert!(compressible("application/json"));
        assert!(compressible("text/html; charset=utf-8"));
        assert!(compressible("application/vnd.api+json"));
        assert!(!compressible("text/event-stream"));
        assert!(!compressible("image/png"));
        assert!(!compressible("application/octet-stream"));
    }

    #[test]
    fn streaming_paths_are_skipped() {
        assert!(streaming_path(&Method::GET, "/event"));
        assert!(streaming_path(&Method::POST, "/session/ses_1/message"));
        assert!(!streaming_path(&Method::GET, "/session/ses_1/message"));
    }
}
