//! Purpose: serve generated web assets or stream the real web UI upstream through the host.
//! Scope: immutable embedded manifest, proxy requests, decoded responses and HTML CSP.
//! Uses:
//! rust/crates/
//! |-- runtime/src/process/options.rs
//! `-- server/src/services/ports.rs
//! External:
//! |-- axum::body::Body
//! |-- reqwest::Client
//! |-- regex::Regex
//! `-- sha2::Sha256
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: request paths never become filesystem paths; proxy bodies remain owned by the response.
//! Contract tests: runtime/tests/ui_assets.rs

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::LazyLock;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, Response, StatusCode};
use base64::Engine;
use futures::StreamExt;
use opencode_server::services::{PortError, UiAssets, UiRequest, UiResponse};
use regex::Regex;
use sha2::{Digest, Sha256};

pub(crate) struct Assets {
    pub client: reqwest::Client,
    pub upstream: String,
    pub embedded: Option<BTreeMap<String, PathBuf>>,
}

#[async_trait]
impl UiAssets for Assets {
    async fn get(&self, path: &str) -> Option<(String, Vec<u8>, Option<String>)> {
        let response = self
            .request(Request::builder().uri(path).body(Body::empty()).ok()?)
            .await
            .ok()?;
        if response.status() == StatusCode::NOT_FOUND {
            return None;
        }
        let mime = response
            .headers()
            .get("content-type")?
            .to_str()
            .ok()?
            .to_owned();
        let policy = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .ok()?;
        Some((mime, body.to_vec(), policy))
    }

    async fn request(&self, request: UiRequest) -> Result<UiResponse, PortError> {
        if let Some(manifest) = &self.embedded {
            let file = manifest
                .get(request.uri().path().trim_start_matches('/'))
                .or_else(|| manifest.get("index.html"));
            let Some(file) = file else {
                return not_found();
            };
            let body = match tokio::fs::read(file).await {
                Ok(body) => body,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return not_found(),
                Err(error) => return Err(port(error)),
            };
            let mime = mime_guess::from_path(file)
                .first_or_octet_stream()
                .to_string();
            let mut response = Response::builder().header("content-type", &mime);
            if mime.starts_with("text/html") {
                response = response.header(
                    "content-security-policy",
                    csp(&String::from_utf8_lossy(&body)),
                );
            }
            return response.body(Body::from(body)).map_err(port);
        }

        let mut target = reqwest::Url::parse(&self.upstream).map_err(port)?;
        target.set_path(request.uri().path());
        target.set_query(None);
        target.set_fragment(None);
        let authority = target.authority().to_owned();
        let (parts, body) = request.into_parts();
        let mut headers = parts.headers;
        for name in [
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
            "accept-encoding",
            "x-opencode-directory",
            "x-opencode-workspace",
        ] {
            headers.remove(name);
        }
        headers.insert("host", HeaderValue::from_str(&authority).map_err(port)?);
        let method = parts.method;
        let outgoing = self.client.request(method.clone(), target).headers(headers);
        let outgoing = if method == Method::GET || method == Method::HEAD {
            outgoing
        } else {
            outgoing.body(reqwest::Body::wrap_stream(body.into_data_stream()))
        };
        let upstream = outgoing.send().await.map_err(port)?;
        let status = upstream.status();
        let mut headers = upstream.headers().clone();
        for name in ["content-encoding", "content-length", "transfer-encoding"] {
            headers.remove(name);
        }
        let html = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/html"));
        let body = if html {
            let bytes = upstream.bytes().await.map_err(port)?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            headers.insert(
                "content-security-policy",
                HeaderValue::from_str(&csp(&text)).map_err(port)?,
            );
            Body::from(text)
        } else {
            headers.insert(
                "content-security-policy",
                HeaderValue::from_str(&csp("")).map_err(port)?,
            );
            // The baseline ends a non-HTML stream on upstream failure after headers were sent.
            Body::from_stream(upstream.bytes_stream().scan((), |_, item| {
                futures::future::ready(item.ok().map(Ok::<_, Infallible>))
            }))
        };
        let mut response = Response::new(body);
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        Ok(response)
    }
}

fn csp(html: &str) -> String {
    static SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<script\b([^>]*)>(.*?)</script>").expect("static script pattern")
    });
    static ID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\bid=(?:"oc-theme-preload-script"|'oc-theme-preload-script')"#)
            .expect("static theme ID pattern")
    });
    static SRC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bsrc\s*=").expect("static script source pattern"));
    let hash = SCRIPT
        .captures_iter(html)
        .find(|capture| ID.is_match(&capture[1]) && !SRC.is_match(&capture[1]))
        .map(|capture| {
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(capture[2].as_bytes()))
        })
        .map(|hash| format!(" 'sha256-{hash}'"))
        .unwrap_or_default();
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hash}; style-src 'self' 'unsafe-inline'; img-src 'self' data: https: blob:; font-src 'self' data:; media-src 'self' data:; connect-src * data: blob:"
    )
}

fn not_found() -> Result<UiResponse, PortError> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"error":"Not Found"}"#))
        .map_err(port)
}

fn port(error: impl std::fmt::Display) -> PortError {
    PortError {
        message: error.to_string(),
    }
}
