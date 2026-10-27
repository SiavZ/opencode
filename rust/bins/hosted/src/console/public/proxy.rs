//! Purpose: proxy baseline docs/enterprise requests and resolve desktop download redirects.
//! Scope: fixed-upstream public HTTP handlers.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/public.rs
//! `-- hosted/src/console/public/locale.rs
//! External:
//! |-- axum::{body::Body, extract::Request}
//! `-- reqwest::Client
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: preserve request query/method/body and response cookies; request Host never chooses upstream.
//! Contract tests: public/tests.rs

use super::{ConsoleState, Error, failure, locale, origin, redirect};
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

pub async fn stats(State(state): State<ConsoleState>, request: Request) -> Result<Response, Error> {
    let mut url = origin(&state)?;
    url.set_path(&format!(
        "/data{}",
        request.uri().path().strip_prefix("/stats").unwrap_or("")
    ));
    url.set_query(request.uri().query());
    Ok(redirect(url.as_str(), StatusCode::PERMANENT_REDIRECT))
}

pub async fn forward(
    State(state): State<ConsoleState>,
    request: Request,
) -> Result<Response, Error> {
    let enterprise = request.uri().path().starts_with("/t/");
    let selected = locale::select(request.headers(), request.uri().path());
    let path = if enterprise {
        format!("/{}", request.uri().path())
    } else {
        let path = if request.uri().path().starts_with("/s/") {
            format!("/docs{}", request.uri().path())
        } else {
            request.uri().path().into()
        };
        locale::docs(selected, &path)
    };
    let base = if enterprise {
        state.config.public.enterprise_origin.as_str()
    } else {
        state.config.public.docs_origin.as_deref().unwrap_or(
            if state.config.stage == "production" {
                "https://docs.opencode.ai"
            } else {
                "https://docs.dev.opencode.ai"
            },
        )
    };
    let mut url = url::Url::parse(base)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "Invalid proxy origin"))?;
    url.set_path(&path);
    url.set_query(request.uri().query());
    let mut headers = request.headers().clone();
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);
    headers.remove(header::CONNECTION);
    if let Some(value) = if enterprise {
        locale::explicit(&headers)
    } else {
        Some(selected)
    } {
        headers.insert(header::ACCEPT_LANGUAGE, locale::tag(value).parse().unwrap());
    }
    let method = request.method().clone();
    let body = reqwest::Body::wrap_stream(request.into_body().into_data_stream());
    let request = state
        .config
        .public
        .http
        .request(method.clone(), url)
        .headers(headers);
    let request = if matches!(method, axum::http::Method::GET | axum::http::Method::HEAD) {
        request
    } else {
        request.body(body)
    };
    let response = request
        .send()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Proxy upstream request failed"))?;
    let status = response.status();
    let mut headers = response.headers().clone();
    let body = Body::from_stream(response.bytes_stream());
    if !enterprise {
        headers.append(
            header::SET_COOKIE,
            locale::cookie(selected).parse().unwrap(),
        );
    }
    let mut response = (status, body).into_response();
    *response.headers_mut() = headers;
    Ok(response)
}

pub async fn download(
    State(state): State<ConsoleState>,
    Path((channel, platform)): Path<(String, String)>,
) -> Result<Response, Error> {
    let asset = match platform.as_str() {
        "darwin-aarch64-dmg" => "opencode-desktop-mac-arm64.dmg",
        "darwin-x64-dmg" => "opencode-desktop-mac-x64.dmg",
        "windows-x64-nsis" => "opencode-desktop-win-x64.exe",
        "linux-x64-deb" => "opencode-desktop-linux-amd64.deb",
        "linux-x64-appimage" => "opencode-desktop-linux-x86_64.AppImage",
        "linux-x64-rpm" => "opencode-desktop-linux-x86_64.rpm",
        _ => return Ok(StatusCode::NOT_FOUND.into_response()),
    };
    let channel = if channel == "stable" {
        "latest"
    } else {
        "beta"
    };
    let url = format!(
        "{}/update/api/{channel}/desktop/opencode",
        state.config.public.update_origin.trim_end_matches('/')
    );
    let response = state
        .config
        .public
        .http
        .get(url)
        .send()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Update upstream request failed"))?;
    if !response.status().is_success() {
        return Ok(response.status().into_response());
    }
    let release: serde_json::Value = response
        .json()
        .await
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "Invalid update upstream JSON"))?;
    let Some(location) = release
        .get("metadata")
        .and_then(|value| value.get("files"))
        .and_then(|value| value.get(asset))
        .and_then(|value| value.get("url"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(StatusCode::BAD_GATEWAY.into_response());
    };
    Ok(redirect(location, StatusCode::FOUND))
}
