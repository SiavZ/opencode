//! Purpose: fetch and transform release notes to the console changelog JSON contract.
//! Scope: fixed GitHub release upstream with best-effort five-minute cache.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! External:
//! |-- regex::Regex
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: failed upstream returns empty releases with 503 and error cache policy.
//! Contract tests: public/changelog_tests.rs

use super::ConsoleState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

fn response(value: Value, ok: bool) -> Response {
    let mut response = (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        axum::Json(value),
    )
        .into_response();
    response.headers_mut().insert(
        "cache-control",
        if ok {
            "public, max-age=1, s-maxage=300, stale-while-revalidate=86400, stale-if-error=86400"
        } else {
            "public, max-age=1, s-maxage=60, stale-while-revalidate=600, stale-if-error=86400"
        }
        .parse()
        .unwrap(),
    );
    cors(&mut response);
    response
}
fn cors(response: &mut Response) {
    for (name, value) in [
        ("access-control-allow-origin", "*"),
        ("access-control-allow-methods", "GET, OPTIONS"),
        (
            "access-control-allow-headers",
            "Content-Type, Authorization",
        ),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
}
pub async fn options() -> Response {
    let mut response = StatusCode::OK.into_response();
    cors(&mut response);
    response
}

pub async fn get(State(state): State<ConsoleState>) -> Response {
    let key = format!("changelog:{}", state.config.public.releases_url);
    if let Some(response) = state.config.public.cache.get(&key) {
        return response;
    }
    let result = async {
        let response = state
            .config
            .public
            .http
            .get(&state.config.public.releases_url)
            .header("accept", "application/vnd.github.v3+json")
            .header("user-agent", "OpenCode-Console")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let data: Value = response.json().await.ok()?;
        let releases = data
            .as_array()?
            .iter()
            .map(|release| {
                let parsed = parse(release.get("body").and_then(Value::as_str).unwrap_or(""));
                let mut value = json!({"highlights": parsed.0, "sections": parsed.1});
                for (source, target) in [
                    ("tag_name", "tag"),
                    ("name", "name"),
                    ("published_at", "date"),
                    ("html_url", "url"),
                ] {
                    if let Some(field) = release.get(source) {
                        value[target] = field.clone();
                    }
                }
                value
            })
            .collect::<Vec<_>>();
        Some(json!({"releases": releases}))
    }
    .await;
    let Some(value) = result else {
        return response(json!({"releases": []}), false);
    };
    let body = serde_json::to_vec(&value).expect("JSON release response");
    let response = response(value, true);
    state
        .config
        .public
        .cache
        .put(key, StatusCode::OK, response.headers().clone(), body, 300);
    response
}

fn parse(body: &str) -> (Vec<Value>, Vec<Value>) {
    use std::sync::LazyLock;
    static HIGHLIGHT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?s)<highlight\s+source="([^"]+)">(.*?)</highlight>"#).unwrap()
    });
    static TITLE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<h2>([^<]+)</h2>").unwrap());
    static PARAGRAPH: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"<p(?:\s+short="([^"]*)")?>([^<]+)</p>"#).unwrap());
    static IMAGE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"<img\s+width="([^"]+)"\s+height="([^"]+)"\s+alt="[^"]*"\s+src="([^"]+)""#,
        )
        .unwrap()
    });
    static VIDEO: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?m)^\s*(https://github\.com/user-attachments/assets/[a-f0-9-]+)\s*$")
            .unwrap()
    });
    let mut highlights: Vec<Value> = Vec::new();
    for group in HIGHLIGHT.captures_iter(body) {
        let content = &group[2];
        let Some(title) = TITLE.captures(content) else {
            continue;
        };
        let media = VIDEO.captures(content).map(|value| json!({"type": "video", "src": &value[1]})).or_else(|| IMAGE.captures(content).map(|value| json!({"type": "image", "src": &value[3], "width": &value[1], "height": &value[2]})));
        let Some(media) = media else {
            continue;
        };
        let paragraph = PARAGRAPH.captures(content);
        let mut item = json!({"title": &title[1], "description": paragraph.as_ref().map(|value| &value[2]).unwrap_or(""), "media": media});
        if let Some(short) = paragraph.as_ref().and_then(|value| value.get(1)) {
            item["shortDescription"] = short.as_str().into();
        }
        if let Some(existing) = highlights
            .iter_mut()
            .find(|value| value["source"] == group[1])
        {
            existing["items"].as_array_mut().unwrap().push(item);
        } else {
            highlights.push(json!({"source": &group[1], "items": [item]}));
        }
    }
    let mut sections: Vec<Value> = Vec::new();
    let mut current: Option<Value> = None;
    let mut skip = false;
    for line in body.split('\n') {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(value) = current.take() {
                sections.push(value);
            }
            current = Some(json!({"title": title.trim(), "items": []}));
            skip = false;
            continue;
        }
        if line.starts_with("**Thank you") {
            skip = true;
            continue;
        }
        if let Some(item) = line.strip_prefix("- ").filter(|_| !skip)
            && let Some(value) = current.as_mut()
        {
            value["items"]
                .as_array_mut()
                .unwrap()
                .push(item.trim().into());
        }
    }
    if let Some(value) = current {
        sections.push(value);
    }
    (highlights, sections)
}
