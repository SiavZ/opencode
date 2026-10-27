//! Purpose: localize known native Zen gateway errors using actual console dictionaries.
//! Scope: pure English-template capture, locale selection and placeholder-preserving rendering.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/zen/locale/selection.rs
//! `-- hosted/src/console/zen/locale/tests.rs
//! External:
//! |-- http::HeaderMap
//! |-- regex::Regex
//! |-- serde_json::Value
//! `-- std::sync::OnceLock
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: unknown text stays unchanged; caller must never translate upstream provider bodies.
//! Contract tests: hosted/src/console/zen/locale/tests.rs

mod selection;
#[cfg(test)]
mod tests;

use http::HeaderMap;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

struct Template {
    key: String,
    pattern: Regex,
    parameters: Vec<String>,
}

/// Only the gateway's own errors may enter this function, never provider error messages.
pub fn translate(message: &str, headers: &HeaderMap, path: &str) -> String {
    let locale = selection::select(headers, path);
    if locale == "en" {
        return message.to_owned();
    }
    static DICTIONARIES: OnceLock<Value> = OnceLock::new();
    let dictionaries = DICTIONARIES.get_or_init(|| {
        serde_json::from_str(include_str!("locale/dictionaries.json"))
            .expect("captured Zen dictionaries must be valid JSON")
    });
    static PLACEHOLDERS: OnceLock<Regex> = OnceLock::new();
    let placeholders = PLACEHOLDERS
        .get_or_init(|| Regex::new(r"\{\{(\w+)\}\}").expect("static placeholder regex"));
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    let templates = TEMPLATES.get_or_init(|| {
        let mut templates = dictionaries["en"]
            .as_object()
            .expect("captured English dictionary")
            .iter()
            .map(|(key, value)| {
                let template = value.as_str().expect("captured string dictionary");
                let mut pattern = String::from("(?s)\\A");
                let mut parameters = Vec::new();
                let mut end = 0;
                for capture in placeholders.captures_iter(template) {
                    let full = capture.get(0).unwrap();
                    pattern.push_str(&regex::escape(&template[end..full.start()]));
                    pattern.push_str("(.*?)");
                    parameters.push(capture[1].to_owned());
                    end = full.end();
                }
                pattern.push_str(&regex::escape(&template[end..]));
                pattern.push_str("\\z");
                Template {
                    key: key.clone(),
                    pattern: Regex::new(&pattern).expect("escaped English template"),
                    parameters,
                }
            })
            .collect::<Vec<_>>();
        // A general model template must not consume a format-specific model message.
        templates.sort_by_key(|template| std::cmp::Reverse(template.pattern.as_str().len()));
        templates
    });
    for template in templates {
        let Some(captures) = template.pattern.captures(message) else {
            continue;
        };
        let localized = dictionaries[locale][&template.key]
            .as_str()
            .or_else(|| dictionaries["en"][&template.key].as_str())
            .expect("English fallback dictionary key");
        return placeholders
            .replace_all(localized, |placeholder: &regex::Captures<'_>| {
                template
                    .parameters
                    .iter()
                    .position(|name| name == &placeholder[1])
                    .and_then(|index| captures.get(index + 1))
                    .map(|value| value.as_str().to_owned())
                    .unwrap_or_else(|| placeholder[0].to_owned())
            })
            .into_owned();
    }
    message.to_owned()
}
