//! Purpose: exact model-family effort and thinking predicates from legacy provider/transform.ts.
//! Scope: pure explicit model metadata, no network or environment.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/providers/variants.rs
//! External:
//! |-- regex::Regex
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/variants.rs
//! Invariants: model version regexes never treat release dates as minor versions.
//! Contract tests: provider-adapters/tests/providers_variants.rs

use serde_json::{Value, json};

pub(super) const WIDE: &[&str] = &["low", "medium", "high"];
pub(super) const ALL: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];
pub(super) const ENCRYPTED: &[&str] = &["reasoning.encrypted_content"];

pub(super) fn text<'a>(model: &'a Value, pointer: &str) -> &'a str {
    model
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn matches(pattern: &str, id: &str) -> bool {
    regex::Regex::new(pattern)
        .expect("static model predicate")
        .is_match(id)
}

fn version(id: &str) -> Option<u32> {
    regex::Regex::new(r"(?:^|/)gpt-5[.-](\d+)(?:[.-]|$)")
        .expect("static model version")
        .captures(id)
        .and_then(|capture| capture[1].parse().ok())
}

fn specific(id: &str) -> Option<Vec<&'static str>> {
    let family = matches(r"(?:^|/)gpt-5(?:[.-]|$)", id);
    if family && id.contains("-chat") {
        return Some(if version(id).is_some() {
            vec!["medium"]
        } else {
            vec![]
        });
    }
    if matches(r"(?:^|/)gpt-5[.-]?pro(?:[.-]|$)", id) {
        return Some(vec!["high"]);
    }
    if family && id.contains("codex") {
        return Some(if version(id).is_some_and(|value| value >= 3) {
            vec!["none", "low", "medium", "high", "xhigh"]
        } else if id.contains("codex-max") || version(id).is_some_and(|value| value >= 2) {
            vec!["low", "medium", "high", "xhigh"]
        } else {
            WIDE.to_vec()
        });
    }
    if matches(r"(?:^|/)gpt-5[.-]\d+[.-]pro(?:[.-]|$)", id) {
        return Some(vec!["medium", "high", "xhigh"]);
    }
    version(id).map(|value| {
        if value == 1 {
            vec!["none", "low", "medium", "high"]
        } else {
            vec!["none", "low", "medium", "high", "xhigh"]
        }
    })
}

pub(super) fn openai(id: &str, release: &str, compatible: bool) -> Vec<&'static str> {
    let id = id.to_lowercase();
    if !compatible && id.contains("deep-research") {
        return vec!["medium"];
    }
    if let Some(values) = specific(&id) {
        return values;
    }
    if compatible {
        return ALL.to_vec();
    }
    let mut values = Vec::new();
    if release >= "2025-11-13" {
        values.push("none");
    }
    if matches(r"(?:^|/)gpt-5(?:[.-]|$)", &id) {
        values.push("minimal");
    }
    values.extend(WIDE);
    if release >= "2025-12-04" {
        values.push("xhigh");
    }
    values
}

pub(super) fn modern(id: &str) -> bool {
    if !id.to_lowercase().contains("claude-") {
        return false;
    }
    let regex = regex::Regex::new(r"(?i)claude-(?:[a-z]+-)?(\d+)(?:[.-](\d{1,2}))?(?:[.@-]|$)")
        .expect("static Claude version");
    let Some(version) = regex.captures(id) else {
        return true;
    };
    let major = version[1].parse::<u32>().unwrap_or_default();
    let minor = version
        .get(2)
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or_default();
    major > 4 || (major == 4 && minor >= 7)
}

pub(super) fn adaptive(id: &str) -> Option<Vec<&'static str>> {
    if modern(id) {
        return Some(vec!["low", "medium", "high", "xhigh", "max"]);
    }
    [
        "opus-4-6",
        "opus-4.6",
        "4-6-opus",
        "4.6-opus",
        "sonnet-4-6",
        "sonnet-4.6",
        "4-6-sonnet",
        "4.6-sonnet",
    ]
    .iter()
    .any(|value| id.contains(value))
    .then(|| vec!["low", "medium", "high", "max"])
}

pub(super) fn opus45(id: &str) -> bool {
    id.contains("opus-4-5") || id.contains("opus-4.5")
}

pub(super) fn kimi(model: &Value) -> bool {
    [text(model, "/providerID"), text(model, "/api/id")]
        .iter()
        .any(|value| {
            let id = value.to_lowercase();
            id.contains("kimi") || id.contains("moonshot")
        })
        || [
            "api.kimi.com",
            "api.moonshot.ai",
            "api.moonshot.cn",
            "api.moonshotai.cn",
        ]
        .iter()
        .any(|host| text(model, "/api/url").to_lowercase().contains(host))
}

pub(super) fn anthropic(model: &Value, effort: &str) -> Option<Value> {
    let id = text(model, "/api/id");
    if opus45(id) {
        let output = model
            .pointer("/limit/output")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        return Some(
            json!({"thinking":{"type":"enabled","budgetTokens":(16000.0_f64.min((output/2.0-1.0).floor())) as i64},"effort":effort}),
        );
    }
    if kimi(model) {
        return Some(
            json!({"thinking":{"type":"adaptive","display":"summarized"},"effort":effort}),
        );
    }
    adaptive(id)?;
    let mut value = json!({"thinking":{"type":"adaptive"},"effort":effort});
    if modern(id) {
        value["thinking"]["display"] = json!("summarized");
    }
    Some(value)
}

pub(super) fn gemini25(id: &str) -> bool {
    matches(r"(?i)gemini-2[.-]5(?:[.-]|$)", id)
}

pub(super) fn google(model: &Value) -> serde_json::Map<String, Value> {
    let id = text(model, "/api/id").to_lowercase();
    if gemini25(&id) {
        let maximum = if id.contains("pro") && !id.contains("flash") {
            32768
        } else {
            24576
        };
        return [
            (
                "high".into(),
                json!({"thinkingConfig":{"includeThoughts":true,"thinkingBudget":16000}}),
            ),
            (
                "max".into(),
                json!({"thinkingConfig":{"includeThoughts":true,"thinkingBudget":maximum}}),
            ),
        ]
        .into_iter()
        .collect();
    }
    let efforts = if id.contains("gemma") {
        vec!["minimal", "high"]
    } else if matches(r"gemini-(?:(?:flash|pro)-)?[12](?:[.-]|$)", &id) {
        vec!["low", "high"]
    } else if id.contains("flash-image") {
        vec!["minimal", "high"]
    } else if id.contains("pro-image") {
        vec!["high"]
    } else if id.contains("flash") {
        vec!["minimal", "low", "medium", "high"]
    } else {
        vec!["low", "medium", "high"]
    };
    efforts
        .into_iter()
        .map(|effort| {
            (
                effort.into(),
                json!({"thinkingConfig":{"includeThoughts":true,"thinkingLevel":effort}}),
            )
        })
        .collect()
}
