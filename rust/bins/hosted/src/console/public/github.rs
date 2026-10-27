//! Purpose: return console GitHub stars, latest release and contributor count.
//! Scope: fixed repository metadata, no arbitrary URL fetch.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! External:
//! |-- axum::Json
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: missing releases or upstream errors become JSON null like undefined baseline query.
//! Contract tests: public/data_tests.rs

use super::ConsoleState;
use axum::{Json, extract::State};
use serde_json::{Value, json};

pub async fn get(State(state): State<ConsoleState>) -> Json<Value> {
    let result = async {
        let base = state.config.public.github_api_base.trim_end_matches('/');
        let request = |url: String| state.config.public.http.get(url).header("user-agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36").send();
        let (meta, releases, contributors) = tokio::join!(request(base.to_owned()), request(format!("{base}/releases")), request(format!("{base}/contributors?per_page=1")));
        let meta: Value = meta.ok()?.json().await.ok()?;
        let releases: Value = releases.ok()?.json().await.ok()?;
        let release = releases.as_array()?.first()?;
        let contributors = contributors.ok()?;
        let count = contributors.headers().get("link").and_then(|value| value.to_str().ok()).and_then(|value| {
            regex::Regex::new(r#"&page=(\d+)>; rel="last""#).unwrap().captures(value).and_then(|value| value[1].parse::<u64>().ok())
        }).unwrap_or(0);
        let mut value = json!({"release": {}, "contributors": count});
        if let Some(stars) = meta.get("stargazers_count") { value["stars"] = stars.clone(); }
        for (source, target) in [("name", "name"), ("html_url", "url"), ("tag_name", "tag_name")] {
            if let Some(field) = release.get(source) { value["release"][target] = field.clone(); }
        }
        Some(value)
    }.await;
    Json(result.unwrap_or(Value::Null))
}
